//! Scripted end-to-end run of the real application, enabled by setting the
//! `LUMEN_SELFTEST` environment variable to an output directory.
//!
//! It drives the same app methods the UI buttons call, grabs framebuffer
//! screenshots of the rendered UI at key moments and exits with a pass/fail
//! status. It exists so the full GUI flow can be verified without hardware or
//! synthetic mouse input.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use egui::{Context, ScreenshotCallback, ViewportCommand};

use super::{Connection, DockTab, LumenApp};
use crate::camera::FocusDirection;
use crate::engine::Command;
use crate::link::DeviceKind;
use crate::ptp::codes::prop;

pub struct SelfTest {
    dir: PathBuf,
    stage: usize,
    stage_started: Instant,
    shot_wait: Option<(String, Instant)>,
    frames_mark: u64,
    shot_done: (Sender<()>, Receiver<()>),
}

impl SelfTest {
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("LUMEN_SELFTEST")?);
        let _ = std::fs::create_dir_all(&dir);
        Some(Self { dir, stage: 0, stage_started: Instant::now(), shot_wait: None, frames_mark: 0, shot_done: crossbeam_channel::unbounded() })
    }

    pub fn output_dir(&self) -> PathBuf {
        self.dir.join("captures")
    }

    fn advance(&mut self) {
        self.stage += 1;
        self.stage_started = Instant::now();
    }

    fn elapsed(&self) -> Duration {
        self.stage_started.elapsed()
    }

    fn request_shot(&mut self, ctx: &Context, name: &str) {
        let (path, done) = (self.dir.join(format!("{name}.png")), self.shot_done.0.clone());
        ctx.send_viewport_cmd(ViewportCommand::Screenshot(ScreenshotCallback::new(move |img| {
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(img.width() as u32, img.height() as u32, rgba) {
                let _ = buf.save(&path);
            }
            let _ = done.send(());
        })));
        self.shot_wait = Some((name.to_owned(), Instant::now()));
    }

    fn fail(&self, why: &str) -> ! {
        let _ = std::fs::write(self.dir.join("selftest.fail"), why);
        eprintln!("SELFTEST FAILED: {why}");
        std::process::exit(1)
    }

    fn save_screenshots(&mut self) {
        if self.shot_done.1.try_recv().is_ok() {
            self.shot_wait = None;
            self.advance();
        }
        if let Some((name, at)) = &self.shot_wait
            && at.elapsed() > Duration::from_secs(10)
        {
            self.fail(&format!("screenshot '{name}' never arrived"));
        }
    }
}

impl LumenApp {
    pub(super) fn selftest_tick(&mut self, ctx: &Context) {
        let Some(mut st) = self.selftest.take() else { return };
        ctx.request_repaint_after(Duration::from_millis(50));
        st.save_screenshots();
        if st.shot_wait.is_none() {
            self.selftest_stage(ctx, &mut st);
        }
        self.selftest = Some(st);
    }

    fn selftest_stage(&mut self, ctx: &Context, st: &mut SelfTest) {
        let timeout = |st: &SelfTest, secs: u64, what: &str| {
            if st.elapsed() > Duration::from_secs(secs) {
                st.fail(&format!("stage {} timed out waiting for {what}", st.stage));
            }
        };
        match st.stage {
            0 => {
                timeout(st, 30, "the device scan to list the virtual camera");
                let real = std::env::var_os("LUMEN_SELFTEST_REAL").is_some();
                if let Some(i) = self.devices.iter().position(|d| (d.kind == DeviceKind::Simulated) != real) {
                    self.cfg.output_dir = st.output_dir();
                    self.selected_device = i;
                    self.connect_selected();
                    st.advance();
                }
            }
            40 => {
                // Audio-only run: let an external tone play, then capture the meters.
                if st.elapsed() > Duration::from_millis(3500) {
                    st.request_shot(ctx, "06-audio");
                    st.stage = 99;
                }
            }
            1 => {
                timeout(st, 30, "connection and live frames");
                if self.is_connected() && self.sharpness.len() >= 8 && !self.settings.is_empty() {
                    if std::env::var_os("LUMEN_SELFTEST_AUDIO").is_some() {
                        if let Some(device) = self.audio_devices.iter().find(|d| d.starts_with(crate::audio::LOOPBACK_PREFIX)).cloned() {
                            self.cfg.audio_device = Some(device);
                            self.start_audio();
                        }
                        st.stage = 39; // +1 after the first screenshot
                    }
                    st.request_shot(ctx, "01-live");
                    if std::env::var_os("LUMEN_SELFTEST_REAL").is_some() {
                        // Real hardware: never take stills; optionally exercise video recording.
                        st.stage = if std::env::var_os("LUMEN_SELFTEST_SHOOT").is_some() {
                            29
                        } else if std::env::var_os("LUMEN_SELFTEST_RECORD").is_some() {
                            19
                        } else {
                            8
                        }; // +1 after the screenshot
                    }
                }
            }
            2 => {
                if let Some(device) = self.audio_devices.first().cloned() {
                    self.cfg.audio_device = Some(device);
                    self.start_audio();
                }
                let iso = self.settings.iter().find(|s| s.prop == prop::EXPOSURE_INDEX).and_then(|s| s.neighbour(2));
                if let Some(v) = iso {
                    self.engine.send(Command::SetSetting { prop: prop::EXPOSURE_INDEX, value: v });
                }
                for _ in 0..14 {
                    self.engine.send(Command::Focus { dir: FocusDirection::Far, steps: 40 });
                }
                self.cfg.show_grid = true;
                st.advance();
            }
            3 => {
                if st.elapsed() > Duration::from_millis(2500) {
                    st.request_shot(ctx, "02-defocused-iso-up");
                }
            }
            4 => {
                self.engine.send(Command::Autofocus);
                self.cfg.show_grid = false;
                st.advance();
            }
            5 => {
                if st.elapsed() > Duration::from_secs(1) {
                    self.capture();
                    st.advance();
                }
            }
            6 => {
                timeout(st, 30, "single capture");
                if self.captures.len() == 1 {
                    self.cfg.bracket_shots = 8;
                    self.cfg.bracket_step = 110;
                    self.engine.send(Command::Focus { dir: FocusDirection::Near, steps: 450 });
                    self.start_bracket();
                    st.advance();
                }
            }
            7 => {
                timeout(st, 150, "bracket + stack");
                if self.stack.result.is_some() {
                    self.dock_tab = DockTab::Captures;
                    st.request_shot(ctx, "03-stacked");
                }
            }
            8 => {
                self.dock_tab = DockTab::Activity;
                if st.elapsed() > Duration::from_millis(600) {
                    st.request_shot(ctx, "04-activity");
                }
            }
            30 => {
                self.cfg.output_dir = st.output_dir();
                self.capture();
                st.advance();
            }
            31 => {
                timeout(st, 60, "the photo to be saved");
                if let Some(item) = self.captures.last() {
                    let ok = item.path.as_ref().is_some_and(|p| p.exists());
                    if !ok {
                        st.fail("the capture has no file on disk");
                    }
                    st.request_shot(ctx, "03-captured");
                    st.stage = if std::env::var_os("LUMEN_SELFTEST_RECORD").is_some() { 19 } else { 8 };
                }
            }
            20 => {
                timeout(st, 20, "the camera to confirm recording started");
                if !self.recording {
                    if st.elapsed() < Duration::from_millis(200) || st.elapsed() > Duration::from_secs(2) && st.elapsed().as_millis() % 4000 < 60 {
                        self.toggle_record();
                    }
                } else {
                    st.frames_mark = self.frames_seen;
                    st.advance();
                }
            }
            21 => {
                if st.elapsed() > Duration::from_secs(5) {
                    let got = self.frames_seen - st.frames_mark;
                    if got < 15 {
                        st.fail(&format!("live view froze while recording: only {got} frames in 5 s"));
                    }
                    st.request_shot(ctx, "02-recording");
                }
            }
            22 => {
                timeout(st, 20, "the camera to confirm recording stopped");
                if self.recording {
                    if st.elapsed() < Duration::from_millis(200) {
                        self.toggle_record();
                    }
                } else {
                    st.stage = 8;
                    st.advance();
                }
            }
            9 => {
                self.toggle_fullscreen(ctx);
                st.advance();
            }
            10 => {
                if st.elapsed() > Duration::from_millis(1500) {
                    st.request_shot(ctx, "05-fullscreen");
                }
            }
            11 => {
                self.mode_switch = Some((true, Instant::now() - Duration::from_millis(900)));
                if st.elapsed() > Duration::from_millis(900) {
                    st.request_shot(ctx, "06-mode-switch");
                }
            }
            12 => {
                self.mode_switch = None;
                self.toggle_fullscreen(ctx);
                st.advance();
            }
            _ => {
                self.mode_switch = None;
                let result_ok = self.stack.result.as_ref().is_some_and(|(p, _)| p.exists());
                let real = std::env::var_os("LUMEN_SELFTEST_REAL").is_some() || std::env::var_os("LUMEN_SELFTEST_AUDIO").is_some();
                if !real && !self.audio_devices.is_empty() && !self.audio_on {
                    st.fail("the audio monitor did not start");
                }
                if (!real && self.captures.len() != 9) || (!real && !result_ok) || !matches!(self.connection, Connection::Connected { .. }) {
                    st.fail(&format!("final state wrong: {} captures, stack ok = {result_ok}", self.captures.len()));
                }
                let _ = std::fs::write(st.dir.join("selftest.ok"), "ok");
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }
}
