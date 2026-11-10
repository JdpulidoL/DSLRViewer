//! End-to-end tests: the real engine, camera driver and PTP stack running
//! against the virtual camera over the real container wire format.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::camera::{Camera, CaptureOutcome, FocusDirection, Vendor};
use crate::engine::{BracketPlan, Command, Engine, Event, Level};
use crate::imaging::stack::{Method, Options};
use crate::link::sim::SimLink;
use crate::link::{self, DeviceKind};
use crate::ptp::PtpError;
use crate::ptp::codes::prop;
use crate::ptp::transport::ContainerTransport;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lumen-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sim_camera() -> (Camera, crate::link::sim::Unplug) {
    let (link, unplug) = SimLink::new();
    (Camera::open(Box::new(ContainerTransport::new(link))).unwrap(), unplug)
}

/// Wait for an event matching `pick`, failing on error logs.
fn wait_for<T>(engine: &Engine, secs: u64, mut pick: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "timed out waiting for engine event");
        let event = engine.events.recv_timeout(left).expect("engine stopped");
        if let Event::Log(Level::Error, msg) = &event {
            panic!("engine reported an error: {msg}");
        }
        if let Some(v) = pick(event) {
            return v;
        }
    }
}

#[test]
fn camera_identifies_vendor_and_reads_settings() {
    let (mut cam, _) = sim_camera();
    assert_eq!(cam.identity().vendor, Vendor::Nikon);
    assert!(cam.capabilities().live_view && cam.capabilities().focus_drive && cam.capabilities().autofocus);
    let settings = cam.read_settings();
    let iso = settings.iter().find(|s| s.prop == prop::EXPOSURE_INDEX).expect("ISO present");
    assert_eq!(iso.text, "ISO 400");
    assert!(iso.writable && iso.choices.len() == 8);
    let battery = settings.iter().find(|s| s.prop == prop::BATTERY_LEVEL).unwrap();
    assert!(!battery.writable);
}

#[test]
fn setting_changes_round_trip_and_invalid_values_are_rejected() {
    let (mut cam, _) = sim_camera();
    cam.set_setting(prop::EXPOSURE_INDEX, 1600).unwrap();
    let iso = cam.read_settings().into_iter().find(|s| s.prop == prop::EXPOSURE_INDEX).unwrap();
    assert_eq!(iso.current, 1600);
    assert!(matches!(cam.set_setting(prop::EXPOSURE_INDEX, 777), Err(PtpError::Response { .. })));
    assert!(cam.set_setting(prop::BATTERY_LEVEL, 5).is_err());
}

#[test]
fn live_view_delivers_decodable_frames_and_focus_moves_sharpness() {
    let (mut cam, _) = sim_camera();
    assert!(cam.live_view_frame().unwrap().is_none(), "no frames before live view starts");
    cam.start_live_view().unwrap();
    let sharp_at = |cam: &mut Camera| {
        let image = cam.live_view_frame().unwrap().expect("frame");
        crate::imaging::decode_live(&image, None).unwrap().sharpness
    };
    let centred = sharp_at(&mut cam);
    for _ in 0..12 {
        cam.focus_drive(FocusDirection::Far, 40).unwrap();
    }
    let far = sharp_at(&mut cam);
    assert!(centred > far, "centre chart should lose sharpness: {centred} vs {far}");
    cam.autofocus().unwrap();
    let again = sharp_at(&mut cam);
    assert!((again - centred).abs() / centred < 0.2, "AF should restore focus: {again} vs {centred}");
    cam.stop_live_view().unwrap();
    assert!(!cam.live_view_active());
}

#[test]
fn capture_downloads_a_full_resolution_jpeg_with_camera_filename() {
    let (mut cam, _) = sim_camera();
    let CaptureOutcome::Downloaded(shots) = cam.capture(Duration::from_secs(10)).unwrap() else { panic!("expected a download") };
    let shot = &shots[0];
    assert_eq!(shot.name, "DSC_0001.JPG");
    let img = image::load_from_memory(&shot.bytes).unwrap();
    assert_eq!((img.width(), img.height()), (1800, 1200));
}

#[test]
fn unplugging_surfaces_as_disconnected() {
    let (mut cam, unplug) = sim_camera();
    unplug.pull();
    assert!(matches!(cam.read_settings().len(), 0));
    assert!(matches!(cam.capture(Duration::from_secs(1)), Err(PtpError::Disconnected)));
}

#[test]
fn engine_full_workflow() {
    let engine = Engine::spawn(std::sync::Arc::new(|| {}));
    let out = temp_dir("workflow");

    engine.send(Command::Scan);
    let devices = wait_for(&engine, 20, |e| if let Event::Devices(d) = e { Some(d) } else { None });
    let sim = devices.into_iter().find(|d| d.kind == DeviceKind::Simulated).expect("simulator listed");
    engine.send(Command::Connect(sim));
    let settings = wait_for(&engine, 20, |e| if let Event::Connected { settings, .. } = e { Some(settings) } else { None });
    assert!(!settings.is_empty());

    // Live view streams frames and peaking changes pixels.
    engine.send(Command::LiveView(true));
    wait_for(&engine, 10, |e| matches!(e, Event::LiveView(true)).then_some(()));
    let first = wait_for(&engine, 10, |e| if let Event::Frame(f) = e { Some(f) } else { None });
    assert_eq!((first.width, first.height), (720, 480));
    engine.send(Command::SetPeaking(Some(crate::imaging::Peaking { threshold: 30.0, color: [0, 255, 0] })));
    let peaked = wait_for(&engine, 10, |e| {
        if let Event::Frame(f) = e { f.rgba.chunks(4).any(|p| p[..3] == [0, 255, 0]).then_some(f) } else { None }
    });
    assert!(peaked.sharpness > 0.0);

    // Settings change is reflected in the next settings snapshot.
    engine.send(Command::SetSetting { prop: prop::EXPOSURE_INDEX, value: 3200 });
    wait_for(&engine, 10, |e| match e {
        Event::Settings(s) => s.iter().any(|x| x.prop == prop::EXPOSURE_INDEX && x.current == 3200).then_some(()),
        _ => None,
    });

    // Single capture lands on disk.
    engine.send(Command::Capture { dir: out.clone(), prefix: "single".into() });
    let rec = wait_for(&engine, 30, |e| if let Event::Captured(r) = e { Some(r) } else { None });
    let path = rec.path.expect("saved path");
    assert!(path.exists() && rec.thumb.is_some());

    // Focus bracket: 6 shots, then stack them.
    let plan = BracketPlan {
        shots: 6,
        step: 120,
        direction: FocusDirection::Far,
        settle_ms: 20,
        output_dir: out.clone(),
        prefix: "stk".into(),
    };
    engine.send(Command::Focus { dir: FocusDirection::Near, steps: 400 });
    engine.send(Command::Bracket(plan));
    let (folder, paths, cancelled) = wait_for(&engine, 120, |e| match e {
        Event::BracketFinished { folder, paths, cancelled } => Some((folder, paths, cancelled)),
        _ => None,
    });
    assert!(!cancelled);
    assert_eq!(paths.len(), 6);
    assert!(folder.starts_with(&out) && paths.iter().all(|p| p.exists()));

    engine.send(Command::Stack { paths: paths.clone(), options: Options { method: Method::Weighted, ..Options::default() } });
    let result = wait_for(&engine, 120, |e| match e {
        Event::StackFinished { path, .. } => Some(path),
        Event::StackFailed(m) => panic!("stack failed: {m}"),
        _ => None,
    });
    assert!(result.exists() && std::fs::metadata(&result).unwrap().len() > 10_000);

    engine.send(Command::Disconnect);
    wait_for(&engine, 10, |e| matches!(e, Event::Disconnected { .. }).then_some(()));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn bracket_can_be_cancelled() {
    let engine = Engine::spawn(std::sync::Arc::new(|| {}));
    let out = temp_dir("cancel");
    let sim = link::scan().into_iter().find(|d| d.kind == DeviceKind::Simulated).unwrap();
    engine.send(Command::Connect(sim));
    wait_for(&engine, 20, |e| matches!(e, Event::Connected { .. }).then_some(()));
    engine.send(Command::Bracket(BracketPlan {
        shots: 50,
        step: 10,
        direction: FocusDirection::Far,
        settle_ms: 0,
        output_dir: out.clone(),
        prefix: "c".into(),
    }));
    wait_for(&engine, 60, |e| matches!(e, Event::BracketProgress { done: 2, .. }).then_some(()));
    engine.send(Command::CancelBracket);
    let (paths, cancelled) = wait_for(&engine, 60, |e| match e {
        Event::BracketFinished { paths, cancelled, .. } => Some((paths, cancelled)),
        _ => None,
    });
    assert!(cancelled && paths.len() < 50);
    let _ = std::fs::remove_dir_all(&out);
}

/// Manual probe for a physically connected camera: `cargo test probe_real -- --ignored --nocapture`.
#[test]
#[ignore]
fn probe_real_camera() {
    let _com = crate::link::wpd::ComGuard::new();
    for d in link::scan() {
        println!("device: {:?} / {} / {}", d.kind, d.name, d.detail);
        if d.kind == DeviceKind::Simulated {
            continue;
        }
        match link::open(&d).and_then(Camera::open) {
            Ok(mut cam) => {
                println!("  opened: {:?} caps {:?}", cam.identity(), cam.capabilities());
                println!("  storage: {}", cam.storage_for_test());
                println!("  state D1B8 {:x?} D11C {:x?} D1B0 {:x?} D1B1 {:x?}", cam.eos_raw(0xD1B8), cam.eos_raw(0xD11C), cam.eos_raw(0xD1B0), cam.eos_raw(0xD1B1));
                if std::env::var_os("LUMEN_PROBE_RECORD").is_some() {
                    println!("  [record scenario] record caps {} live {:?}", cam.capabilities().record, cam.start_live_view());
                    if std::env::var_os("LUMEN_PROBE_STATE").is_some() {
                        let show = |cam: &Camera, tag: &str| println!("  {tag}: D1B8 {:x?} D1B1 {:x?} D1B0 {:x?} video_mode {}", cam.eos_raw(0xD1B8), cam.eos_raw(0xD1B1), cam.eos_raw(0xD1B0), cam.video_mode());
                        show(&cam, "after live start");
                        for code in [0xD1B8u16, 0xD1B1, 0xD105] {
                            println!("  direct {code:04X}: {}", cam.direct_read_for_test(code));
                        }
                        for i in 0..6 {
                            std::thread::sleep(Duration::from_millis(500));
                            cam.poll_settings();
                            show(&cam, &format!("poll {i}"));
                        }
                        println!("  reinit {:?}", cam.reinit_remote());
                        for i in 0..6 {
                            std::thread::sleep(Duration::from_millis(700));
                            cam.poll_settings();
                            show(&cam, &format!("after reinit {i}"));
                        }
                        cam.close();
                        continue;
                    }
                    if std::env::var_os("LUMEN_PROBE_REINIT").is_some() {
                        let t = std::time::Instant::now();
                        println!("  reinit: {:?} in {} ms; caps {:?}; live {}", cam.reinit_remote(), t.elapsed().as_millis(), cam.refresh_capabilities(), cam.live_view_active());
                    }
                    if std::env::var_os("LUMEN_PROBE_CARDONLY").is_some() {
                        println!("  card only: {:?}; SaveTo {:x?} D1B8 {:x?} D1B1 {:x?} video_mode {}", cam.card_only_for_test(), cam.eos_raw(0xD11C), cam.eos_raw(0xD1B8), cam.eos_raw(0xD1B1), cam.video_mode());
                    }
                    if std::env::var_os("LUMEN_PROBE_ROUTED").is_some() {
                        println!("  routed to PC like photo mode: {:?}; SaveTo now {:x?}", cam.route_to_pc_for_test(), cam.eos_raw(0xD11C).map(|x| x.0));
                        let t = std::time::Instant::now();
                        cam.poll_settings();
                        println!("  after poll SaveTo {:x?} ({} ms)", cam.eos_raw(0xD11C).map(|x| x.0), t.elapsed().as_millis());
                    }
                    let t0 = std::time::Instant::now();
                    let (mut ok, mut none, mut err) = (0, 0, 0);
                    let mut started = false;
                    let mut stopped = false;
                    let mut last = 0;
                    while t0.elapsed() < Duration::from_secs(14) {
                        let s = t0.elapsed().as_secs();
                        if s == 3 && !started {
                            started = true;
                            println!("  t={s}s rec start: {:?}", cam.set_recording(true));
                        }
                        if s == 5 && last == 5 && ok + none + err == 0 {
                            cam.poll_settings();
                            println!("  t=5s state D1B0 {:x?} D1B1 {:x?} D1B8 {:x?}", cam.eos_raw(0xD1B0).map(|x| x.0), cam.eos_raw(0xD1B1).map(|x| x.0), cam.eos_raw(0xD1B8).map(|x| x.0));
                        }
                        if s == 9 && !stopped {
                            stopped = true;
                            println!("  t={s}s rec stop: {:?}", cam.set_recording(false));
                        }
                        match cam.live_view_frame() {
                            Ok(Some(_)) => ok += 1,
                            Ok(None) => { none += 1; std::thread::sleep(Duration::from_millis(30)); }
                            Err(e) => {
                                if err == 0 {
                                    println!("  first frame error: {e}");
                                }
                                err += 1;
                                std::thread::sleep(Duration::from_millis(30));
                            }
                        }
                        if s != last {
                            println!("  t={last}s frames ok={ok} none={none} err={err} recording={}", cam.recording());
                            (ok, none, err) = (0, 0, 0);
                            last = s;
                        }
                    }
                    println!("  stop lv {:?}", cam.stop_live_view());
                    cam.close();
                    continue;
                }
                let i = cam.device_info();
                println!("  ext id {:#x} desc {:?} ops {:04x?}", i.vendor_extension_id, i.vendor_extension_desc, i.operations);
                println!("  props {:04x?}", i.properties);
                {
                    let s = cam.read_settings();
                    if let Some(iso) = s.iter().find(|x| x.prop == 0xD103) {
                        println!("  write-phase test (set ISO to its current value): {:?}", cam.set_setting(0xD103, iso.current));
                    }
                    println!("  drive lens near (no LV): {:?}", cam.focus_drive(FocusDirection::Near, 1));
                }
                for code in 0xD100u16..=0xD1FF {
                    if let Some(v) = cam.eos_raw(code) {
                        println!("  eos {code:#x}: {:x?}", v);
                    }
                }
                if cam.capabilities().live_view {
                    println!("  live view start: {:?}", cam.start_live_view());
                    let mut got = 0;
                    for _ in 0..40 {
                        match cam.live_view_frame() {
                            Ok(Some(img)) => {
                                got += 1;
                                if got == 1 {
                                    println!("  first frame decodes: {}", crate::imaging::decode_live(&img, None).is_ok());
                                }
                            }
                            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                            Err(e) => {
                                println!("  frame error: {e}");
                                break;
                            }
                        }
                    }
                    println!("  frames received: {got}");
                    println!("  stop: {:?}", cam.stop_live_view());
                }
                println!("  settings: {:?}", cam.read_settings().iter().map(|s| (s.label, s.text.clone())).collect::<Vec<_>>());
                if let Some(out) = std::env::var_os("LUMEN_PROBE_BODYSHOT") {
                    println!("  body-shot: trigger status {:?}", cam.release_only_for_test());
                    let t0 = std::time::Instant::now();
                    let mut got = 0;
                    while t0.elapsed() < Duration::from_secs(20) && got == 0 {
                        std::thread::sleep(Duration::from_millis(700));
                        cam.poll_settings();
                        match cam.take_new_images() {
                            Ok(shots) => {
                                for s in shots {
                                    let p = std::path::Path::new(&out).join(&s.name);
                                    std::fs::write(&p, &s.bytes).unwrap();
                                    println!("  body-shot saved {} ({} bytes)", p.display(), s.bytes.len());
                                    got += 1;
                                }
                            }
                            Err(e) => println!("  body-shot error: {e}"),
                        }
                    }
                    println!("  body-shot images received: {got}");
                }
                if std::env::var_os("LUMEN_PROBE_SHOOT").is_some() {
                    let out = std::env::var("LUMEN_PROBE_SHOOT").unwrap();
                    println!("  capture: firing shutter...");
                    if std::env::var_os("LUMEN_SHOOT_LV").is_some() {
                        println!("  lv: {:?}", cam.start_live_view());
                        std::thread::sleep(Duration::from_millis(1500));
                    }
                    match cam.capture(Duration::from_secs(25)) {
                        Ok(CaptureOutcome::Downloaded(shots)) => {
                            for s in shots {
                                let p = std::path::Path::new(&out).join(&s.name);
                                std::fs::write(&p, &s.bytes).unwrap();
                                println!("  saved {} ({} bytes)", p.display(), s.bytes.len());
                            }
                        }
                        Ok(CaptureOutcome::StoredOnCamera) => println!("  stored on camera card only"),
                        Err(e) => println!("  capture error: {e}"),
                    }
                }
                cam.close();
            }
            Err(e) => println!("  open failed: {e}"),
        }
    }
}

/// `LUMEN_RAW_FILE=<path> cargo test probe_raw -- --ignored --nocapture` reports the embedded preview of a RAW file.
#[test]
#[ignore]
fn probe_raw() {
    let path = std::env::var("LUMEN_RAW_FILE").expect("set LUMEN_RAW_FILE");
    let bytes = std::fs::read(path).unwrap();
    match crate::imaging::raw::embedded_jpeg(&bytes) {
        Some(j) => {
            let img = image::load_from_memory(j).unwrap();
            println!("preview: {} bytes, {}x{}", j.len(), img.width(), img.height());
        }
        None => println!("no embedded preview found"),
    }
}
