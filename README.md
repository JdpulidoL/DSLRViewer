# Lumen Tether Studio

A Rust desktop app for tethered shooting with DSLR/mirrorless cameras over USB:
live view, exposure control, focus drive, focus bracketing and focus stacking.
Built from scratch after studying how Helicon Focus 8 structures this workflow
(see [docs/ANALYSIS.md](docs/ANALYSIS.md)).

## Run

```powershell
cargo run --release
```

No camera? Press **Connect virtual camera** – a built-in DSLR simulator speaks
the real PTP wire protocol, so every feature works end to end.

Shortcuts: `R` record video · `F`/`F11` fullscreen (`Esc` exits) · `I` camera info overlay · `Space` capture · `L` live view · `A` autofocus · `←/→` focus
nearer/farther (hold to repeat) · `G` grid · `P` peaking.

## Features

* **Connection** – discovers cameras over Windows WPD (stock MTP/PTP driver)
  and raw USB (libusb, for WinUSB-bound cameras); opens a PTP session and
  identifies the vendor extension (Nikon / Canon EOS / generic).
* **Live view** – JPEG frames decoded off-thread at ~30 fps, with focus peaking,
  RGB/luma histogram, rule-of-thirds grid, scroll-to-zoom + drag-to-pan,
  and a focus-sharpness meter with history.
* **Camera controls** – every property the camera advertises is shown with its
  real allowed values (mode, ISO, aperture, shutter, EV, white balance, focus
  mode, metering, battery); changes are validated by the camera and re-read.
* **Focus drive** – nearer/farther in three step sizes with hold-to-repeat, and
  an autofocus trigger where the camera supports it.
* **Focus bracketing** – N shots, configurable step/settle time/direction, live
  progress, cancel at any time, one numbered folder per sequence.
* **Focus stacking** – Weighted or Sharpest merge of a sequence (auto after a
  bracket, or pick any files).
* **Video (Canon EOS)** – start/stop recording over USB with a running clock
  (header, OSD and a pulsing red frame) while the live preview keeps running
  (the body sends uncompressed 720x480 frames in video mode; decoded here),
  battery level, and a camera-style info overlay (mode, shutter, aperture, EV
  scale, ISO, WB, focus mode). Full-screen live view with an auto-hiding toolbar.
  In video mode the shutter button saves a live-view snapshot (the body refuses stills).
* **Audio monitor** – real-time peak meters (L/R, dBFS, peak hold, clip LED) on any
  PC audio input or on system sound (WASAPI loopback), with optional "Listen" playback.
  Cameras send no audio over USB, so use a microphone or the camera audio out into a line-in.
* **Photos** – from the app button/Space or from the camera's own shutter button (both land in the gallery and output
  folder). RAW files (`.CR2`) also get their embedded JPEG preview saved for viewing and stacking.
* **Capture gallery, activity log, toasts**, persisted preferences
  (`%APPDATA%\Lumen\config.json`).

## For AI-assisted development

`CLAUDE.md` (project rules), `docs/ARCHITECTURE.md`, `docs/HARDWARE_NOTES.md` (verified camera findings),
`docs/TESTING.md`, `docs/ROADMAP.md` and `.claude/commands/` (`/check`, `/selftest`, `/probe-camera`, `/new-vendor`).

## Architecture

```
src/
  ptp/      ISO 15740 codec, datasets, session, transport abstraction
  link/     WireLink/transport implementations: usb (libusb), wpd (Windows), sim
  camera/   vendor-aware Camera API (settings, live view, focus, capture)
  engine/   camera thread + typed Command/Event channels, bracketing, files
  audio/    input level monitor (cpal), loopback, listen playback
  imaging/  frame decode, histogram, peaking, sharpness, focus stacking
  app/      egui UI: theme, custom widgets, panels, scripted self-test
```

## Verification

```powershell
cargo test            # 21 tests: codec, datasets, scene, stacking, engine e2e
cargo clippy --all-targets
$env:LUMEN_SELFTEST = "C:\some\dir"; cargo run --release   # scripted GUI run
```

`LUMEN_SELFTEST` makes the real UI connect to the virtual camera, change
settings, defocus/autofocus, capture, run an 8-shot bracket, stack it, save
framebuffer screenshots and exit non-zero on any failure.

## Honest limits

* Only a Canon Rebel T5i has been tried on real hardware. The libusb link and
  the Nikon operations are implemented from public PTP knowledge and are only
  protocol-tested against the simulator. `cargo test probe_real -- --ignored
  --nocapture` probes any attached camera.
* On Windows, cameras bound to the stock driver are reached through WPD; the
  libusb entry only appears for cameras bound to WinUSB (it reports "access
  denied" otherwise, which is why such entries are hidden when WPD sees the
  same device).
* Canon EOS (verified live on a Rebel T5i over WPD): connection, exposure
  settings (via the EOS event model), live view and focus drive work. Shutter
  release is implemented (with download to the PC through `GetPartialObject`)
  but the T5i answered status 0x7 to the trigger in testing, so it is not
  confirmed; the app reports that refusal instead of hanging. Check the lens,
  memory card and shooting mode.
* Video recording and live view while recording are verified on the T5i. The
  photo-mode shutter path (download to the PC) is implemented but not yet verified
  on hardware; see docs/ROADMAP.md.
* The audio monitor was verified with loopback capture; no microphone was attached
  to the test machine.
* Over WPD, cameras are polled for new objects instead of receiving PTP events.
* Cameras must be in PTP / PC-connect mode; some bodies ignore live view over
  USB entirely.
* Stacking has no sub-pixel alignment; keep the camera still.

## Camera modes and diagnostics

* Turning the camera's mode dial between photo and video is detected; a loading screen shows while Lumen re-initialises the camera
  and a PHOTO/VIDEO chip next to LIVE shows the current position.
* Changes made by hand on the camera (ISO, aperture, shutter, ...) appear in the panel within half a second and are highlighted.
* Photos and recordings started with the camera's own buttons are picked up by the app.
* `logs/lumen.log` records everything the app and the camera do; attach it when reporting a problem.
