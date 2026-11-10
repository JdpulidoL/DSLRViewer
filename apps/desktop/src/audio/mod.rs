//! Real-time audio level monitor on a system input device.
//!
//! PTP cameras do not stream sound over USB, so the audio comes from a PC input:
//! a microphone, a line-in fed from the camera's headphone/mic jack, or an audio
//! interface / capture card. `cpal` streams are not `Send`, so everything lives on
//! one dedicated thread driven by [`Command`]s; the UI reads [`Levels`] lock-free.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use crossbeam_channel::{Receiver, Sender};

/// Floor of the meters in dBFS.
pub const FLOOR_DB: f32 = -90.0;

#[derive(Debug)]
pub enum Command {
    Start { device: Option<String> },
    Stop,
    Listen(bool),
    Volume(f32),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Started { device: String, channels: usize, sample_rate: u32 },
    Stopped,
    Error(String),
    /// Listening (monitor playback) could not be enabled.
    ListenUnavailable(String),
}

/// Level state shared between the audio callback and the UI.
#[derive(Default)]
pub struct Levels {
    /// Largest absolute sample since the last `take_peaks`, per channel (f32 bits).
    peak: [AtomicU32; 2],
    /// RMS of the most recent block, per channel (f32 bits).
    rms: [AtomicU32; 2],
    channels: AtomicUsize,
    clipped: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapshot {
    pub peak_db: [f32; 2],
    pub rms_db: [f32; 2],
    pub channels: usize,
    pub clipped: bool,
}

pub fn to_db(linear: f32) -> f32 {
    if linear <= 1e-5 { FLOOR_DB } else { (20.0 * linear.log10()).max(FLOOR_DB) }
}

impl Levels {
    fn publish(&self, peak: [f32; 2], rms: [f32; 2]) {
        for c in 0..2 {
            // Non-negative floats order like their bit patterns, so an integer max works.
            self.peak[c].fetch_max(peak[c].to_bits(), Ordering::Relaxed);
            self.rms[c].store(rms[c].to_bits(), Ordering::Relaxed);
        }
        if peak.iter().any(|p| *p >= 0.999) {
            self.clipped.store(true, Ordering::Relaxed);
        }
    }

    fn reset(&self, channels: usize) {
        for c in 0..2 {
            self.peak[c].store(0, Ordering::Relaxed);
            self.rms[c].store(0, Ordering::Relaxed);
        }
        self.clipped.store(false, Ordering::Relaxed);
        self.channels.store(channels, Ordering::Relaxed);
    }

    /// Read the levels, clearing the peak accumulators (the UI applies its own hold/decay).
    pub fn take(&self) -> Snapshot {
        let load = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        Snapshot {
            peak_db: [to_db(f32::from_bits(self.peak[0].swap(0, Ordering::Relaxed))), to_db(f32::from_bits(self.peak[1].swap(0, Ordering::Relaxed)))],
            rms_db: [to_db(load(&self.rms[0])), to_db(load(&self.rms[1]))],
            channels: self.channels.load(Ordering::Relaxed),
            clipped: self.clipped.load(Ordering::Relaxed),
        }
    }

    pub fn clear_clip(&self) {
        self.clipped.store(false, Ordering::Relaxed);
    }
}

/// Peak and RMS of the first two channels of an interleaved block (mono is mirrored).
pub fn measure(samples: &[f32], channels: usize) -> ([f32; 2], [f32; 2]) {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    if frames == 0 {
        return ([0.0; 2], [0.0; 2]);
    }
    let (mut peak, mut sum) = ([0f32; 2], [0f32; 2]);
    for frame in samples.chunks_exact(channels) {
        for c in 0..2 {
            let v = frame[c.min(channels - 1)];
            peak[c] = peak[c].max(v.abs());
            sum[c] += v * v;
        }
    }
    (peak, [(sum[0] / frames as f32).sqrt(), (sum[1] / frames as f32).sqrt()])
}

/// Prefix of the pseudo input that captures what an output device is playing (WASAPI loopback).
pub const LOOPBACK_PREFIX: &str = "Loopback: ";

/// Names of the available input devices, default first, followed by loopback captures of the outputs.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.description().ok()).map(|d| d.name().to_string());
    let mut names: Vec<String> = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.description().ok().map(|d| d.name().to_string())).collect())
        .unwrap_or_default();
    names.sort();
    names.dedup();
    if let Some(d) = default {
        names.retain(|n| *n != d);
        names.insert(0, d);
    }
    if cfg!(windows) {
        let mut outputs: Vec<String> = host
            .output_devices()
            .map(|it| it.filter_map(|d| d.description().ok().map(|d| format!("{LOOPBACK_PREFIX}{}", d.name()))).collect())
            .unwrap_or_default();
        outputs.sort();
        outputs.dedup();
        // What the default speakers are playing is the most useful loopback: list it first.
        if let Some(default_out) = host.default_output_device().and_then(|d| d.description().ok()).map(|d| format!("{LOOPBACK_PREFIX}{}", d.name())) {
            outputs.retain(|n| *n != default_out);
            outputs.insert(0, default_out);
        }
        names.extend(outputs);
    }
    names
}

pub struct AudioMonitor {
    tx: Sender<Command>,
    pub notices: Receiver<Notice>,
    pub levels: Arc<Levels>,
    handle: Option<JoinHandle<()>>,
}

impl AudioMonitor {
    pub fn spawn(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (ntx, nrx) = crossbeam_channel::unbounded();
        let levels = Arc::new(Levels::default());
        let thread_levels = levels.clone();
        let handle = std::thread::Builder::new()
            .name("audio-monitor".into())
            .spawn(move || Worker::new(thread_levels, ntx, wake).run(rx))
            .expect("failed to start the audio thread");
        Self { tx, notices: nrx, levels, handle: Some(handle) }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }
}

impl Drop for AudioMonitor {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Playback side of "listen": mono samples queued by the input callback.
#[derive(Clone)]
struct ListenBus {
    enabled: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    queue: Arc<Mutex<VecDeque<f32>>>,
}

struct Running {
    _input: Stream,
    _output: Option<Stream>,
    sample_rate: u32,
}

struct Worker {
    levels: Arc<Levels>,
    notices: Sender<Notice>,
    wake: Arc<dyn Fn() + Send + Sync>,
    bus: ListenBus,
    running: Option<Running>,
}

impl Worker {
    fn new(levels: Arc<Levels>, notices: Sender<Notice>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let bus = ListenBus {
            enabled: Arc::new(AtomicBool::new(false)),
            volume: Arc::new(AtomicU32::new(0.8f32.to_bits())),
            queue: Arc::new(Mutex::new(VecDeque::new())),
        };
        Self { levels, notices, wake, bus, running: None }
    }

    fn notify(&self, n: Notice) {
        let _ = self.notices.send(n);
        (self.wake)();
    }

    fn run(mut self, rx: Receiver<Command>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Command::Start { device } => self.start(device.as_deref()),
                Command::Stop => self.stop(),
                Command::Listen(on) => self.listen(on),
                Command::Volume(v) => self.bus.volume.store(v.clamp(0.0, 1.5).to_bits(), Ordering::Relaxed),
                Command::Shutdown => break,
            }
        }
        self.stop();
    }

    fn stop(&mut self) {
        if self.running.take().is_some() {
            self.bus.enabled.store(false, Ordering::Relaxed);
            self.levels.reset(0);
            self.notify(Notice::Stopped);
        }
    }

    fn start(&mut self, name: Option<&str>) {
        self.stop();
        match self.open_input(name) {
            Ok((running, device, channels)) => {
                let rate = running.sample_rate;
                self.running = Some(running);
                self.notify(Notice::Started { device, channels, sample_rate: rate });
            }
            Err(e) => self.notify(Notice::Error(e)),
        }
    }

    fn open_input(&mut self, name: Option<&str>) -> Result<(Running, String, usize), String> {
        let host = cpal::default_host();
        let device = match name {
            Some(n) if n.starts_with(LOOPBACK_PREFIX) => {
                let out = &n[LOOPBACK_PREFIX.len()..];
                host.output_devices()
                    .map_err(|e| e.to_string())?
                    .find(|d| d.description().is_ok_and(|d| d.name() == out))
                    .ok_or_else(|| format!("audio output '{out}' is not available"))?
            }
            Some(n) => host
                .input_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.description().is_ok_and(|d| d.name() == n))
                .ok_or_else(|| format!("audio input '{n}' is not available"))?,
            None => host.default_input_device().ok_or("no audio input device found")?,
        };
        let mut label = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Audio input".into());
        if name.is_some_and(|n| n.starts_with(LOOPBACK_PREFIX)) {
            label.push_str(" (system sound)");
        }
        // An output device captured as an input (loopback) only reports an output config.
        let supported = device.default_input_config().or_else(|_| device.default_output_config()).map_err(|e| format!("{label}: {e}"))?;
        let (channels, rate, format) = (supported.channels() as usize, supported.sample_rate(), supported.sample_format());
        let config = supported.config();
        self.levels.reset(channels);
        self.bus.queue.lock().map_err(|_| "audio queue poisoned")?.clear();
        let (levels, bus) = (self.levels.clone(), self.bus.clone());
        let stream = match format {
            SampleFormat::F32 => build_input::<f32>(&device, config, channels, levels, bus),
            SampleFormat::I16 => build_input::<i16>(&device, config, channels, levels, bus),
            SampleFormat::U16 => build_input::<u16>(&device, config, channels, levels, bus),
            SampleFormat::I32 => build_input::<i32>(&device, config, channels, levels, bus),
            other => Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| format!("{label}: {e}"))?;
        stream.play().map_err(|e| format!("{label}: {e}"))?;
        Ok((Running { _input: stream, _output: None, sample_rate: rate }, label, channels))
    }

    fn listen(&mut self, on: bool) {
        let Some(running) = self.running.as_mut() else { return };
        if !on {
            self.bus.enabled.store(false, Ordering::Relaxed);
            running._output = None;
            return;
        }
        match build_output(running.sample_rate, self.bus.clone()) {
            Ok(stream) => {
                running._output = Some(stream);
                self.bus.enabled.store(true, Ordering::Relaxed);
            }
            Err(e) => self.notify(Notice::ListenUnavailable(e)),
        }
    }
}

fn build_input<T>(device: &cpal::Device, config: StreamConfig, channels: usize, levels: Arc<Levels>, bus: ListenBus) -> Result<Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                scratch.clear();
                scratch.extend(data.iter().map(|s| f32::from_sample(*s)));
                let (peak, rms) = measure(&scratch, channels);
                levels.publish(peak, rms);
                if bus.enabled.load(Ordering::Relaxed) {
                    let volume = f32::from_bits(bus.volume.load(Ordering::Relaxed));
                    if let Ok(mut q) = bus.queue.try_lock() {
                        for frame in scratch.chunks_exact(channels.max(1)) {
                            q.push_back(frame.iter().sum::<f32>() / frame.len() as f32 * volume);
                        }
                        // Keep latency bounded: drop the oldest audio beyond ~0.4 s.
                        let cap = 19_200;
                        while q.len() > cap {
                            q.pop_front();
                        }
                    }
                }
            },
            |e| eprintln!("audio input error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

fn build_output(sample_rate: u32, bus: ListenBus) -> Result<Stream, String> {
    let device = cpal::default_host().default_output_device().ok_or("no audio output device found")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    if supported.sample_format() != SampleFormat::F32 {
        return Err(format!("output format {:?} is not supported for monitoring", supported.sample_format()));
    }
    let mut config = supported.config();
    config.sample_rate = sample_rate;
    let out_channels = config.channels as usize;
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [f32], _| {
                let mut q = bus.queue.try_lock().ok();
                for frame in out.chunks_exact_mut(out_channels.max(1)) {
                    let s = q.as_mut().and_then(|q| q.pop_front()).unwrap_or(0.0).clamp(-1.0, 1.0);
                    frame.fill(s);
                }
            },
            |e| eprintln!("audio output error: {e}"),
            None,
        )
        .map_err(|e| format!("cannot play at {sample_rate} Hz: {e}"))?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_peak_and_rms_per_channel() {
        // Left: full-scale square wave. Right: silence.
        let block: Vec<f32> = (0..200).flat_map(|i| [if i % 2 == 0 { 1.0 } else { -1.0 }, 0.0]).collect();
        let (peak, rms) = measure(&block, 2);
        assert_eq!(peak, [1.0, 0.0]);
        assert!((rms[0] - 1.0).abs() < 1e-6 && rms[1] == 0.0);
        assert!(to_db(rms[0]).abs() < 0.01);
        assert_eq!(to_db(rms[1]), FLOOR_DB);
    }

    #[test]
    fn mono_is_mirrored_and_empty_is_silent() {
        let (peak, _) = measure(&[0.5, -0.25, 0.1], 1);
        assert_eq!(peak, [0.5, 0.5]);
        assert_eq!(measure(&[], 2), ([0.0; 2], [0.0; 2]));
        assert!((to_db(0.5) + 6.02).abs() < 0.01);
    }

    #[test]
    fn peaks_accumulate_until_taken() {
        let l = Levels::default();
        l.reset(2);
        l.publish([0.2, 0.1], [0.1, 0.05]);
        l.publish([0.9, 0.05], [0.3, 0.02]);
        let s = l.take();
        assert!((s.peak_db[0] - to_db(0.9)).abs() < 1e-4 && (s.peak_db[1] - to_db(0.1)).abs() < 1e-4);
        assert_eq!(l.take().peak_db, [FLOOR_DB; 2], "peaks reset after reading");
        l.publish([1.0, 0.0], [0.5, 0.0]);
        assert!(l.take().clipped);
        l.clear_clip();
        assert!(!l.take().clipped);
    }
}

/// Manual probe: `cargo test probe_audio -- --ignored --nocapture` listens to the default input for 2 s.
#[cfg(test)]
#[test]
#[ignore]
fn probe_audio() {
    println!("inputs: {:?}", input_devices());
    let monitor = AudioMonitor::spawn(Arc::new(|| {}));
    monitor.send(Command::Start { device: std::env::var("LUMEN_AUDIO_DEVICE").ok() });
    let mut started = false;
    let t0 = std::time::Instant::now();
    let mut loudest = FLOOR_DB;
    while t0.elapsed() < std::time::Duration::from_secs(3) {
        while let Ok(n) = monitor.notices.try_recv() {
            println!("notice: {n:?}");
            started |= matches!(n, Notice::Started { .. });
        }
        let s = monitor.levels.take();
        loudest = loudest.max(s.peak_db[0]).max(s.peak_db[1]);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    println!("started={started} loudest peak {loudest:.1} dBFS");
    monitor.send(Command::Stop);
}
