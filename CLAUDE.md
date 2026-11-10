# Lumen Tether Studio - guide for AI coding agents

Rust desktop app (egui/eframe, Windows-first) for tethered shooting with DSLRs over USB:
live view, exposure control, focus drive, focus bracketing + stacking, video recording,
camera-style OSD, fullscreen, and a PC audio level monitor.
Built from scratch after studying how Helicon Focus 8 splits camera control (see `docs/ANALYSIS.md`).

Read this file first, then `docs/ARCHITECTURE.md`. Hardware facts that cost a lot to
discover live in `docs/HARDWARE_NOTES.md` - **read it before touching `camera/`, `link/wpd.rs` or live view.**

## Commands

```powershell
$env:Path += ";$env:USERPROFILE\.cargo\bin"      # cargo is not on PATH by default in this setup
cargo build                                       # dev profile is optimised (opt-level 1, deps 3)
cargo test                                        # whole workspace (~39 tests, ~2 s, no hardware needed)
cargo clippy --workspace --all-targets             # must stay warning-free
cargo run --release -p lumen                      # desktop app
```

Slash commands in `.claude/commands/`: `/selftest`, `/probe-camera`, `/new-vendor`, `/check`.

## Layout (one line each; details in docs/ARCHITECTURE.md)

Cargo workspace (monorepo): `crates/lumen-core` (no UI, shared), `crates/lumen-android` (UniFFI bindings),
`apps/desktop` (egui, package `lumen`), `android/` (Gradle project, Kotlin + Jetpack Compose). CI/CD: `.github/workflows/`,
see `docs/RELEASING.md`.

| Path | Responsibility |
|---|---|
| `crates/lumen-core/src/ptp/` | ISO 15740 codec, datasets, `Session`, `PtpTransport`/`WireLink` traits. No vendor logic. |
| `crates/lumen-core/src/link/` | Transports: `wpd.rs` (Windows MTP/PTP driver), `usb.rs` (libusb; desktop only), `register_bridge` (platform-provided pipe, Android), `sim/` (virtual camera speaking real PTP). |
| `crates/lumen-core/src/camera/` | `Camera` API + vendor drivers: Nikon ops, Canon EOS (`eos.rs` property model, `evf.rs` live-view payloads). |
| `crates/lumen-core/src/engine/` | One camera thread (`worker.rs`), typed `Command`/`Event` channels, bracketing, file saving. |
| `crates/lumen-core/src/imaging/` | Frame analysis (histogram, peaking, sharpness) and focus stacking. |
| `crates/lumen-core/src/e2e.rs` | Engine/camera tests against the simulator + ignored hardware probes. |
| `crates/lumen-android/` | `LumenSession` + `LumenListener` exposed to Kotlin via UniFFI (library mode; Gradle generates the Kotlin). |
| `apps/desktop/src/audio/` | cpal input monitor on its own thread, lock-free `Levels`, optional listen playback, WASAPI loopback. |
| `apps/desktop/src/app/` | egui UI. `mod.rs` = state + event handling; panels; `widgets.rs` custom widgets; `selftest.rs` scripted GUI run. |
| `android/app/` | Native Android UI. `UsbCamera.kt` does `UsbManager` permission and passes the fd to Rust; `LumenViewModel.kt` is the only code that talks to `LumenSession`; `ui/LumenApp.kt` switches phone/tablet layout by `WindowWidthSizeClass`. |

Android notes: **USB I/O is done in Kotlin** (`UsbCamera.kt` -> `UsbDeviceConnection.bulkTransfer`) and Rust only frames PTP:
`lumen-android` exposes the foreign trait `UsbTransport`, wraps it in `KotlinLink` and parks it with `link::register_bridge`
(`DeviceKind::Bridge`). Do not go back to libusb over an Android file descriptor: on a Canon T5i every libusb transfer timed
out while `bulkTransfer` answered instantly (see docs/HARDWARE_NOTES.md). The dev machine can build Android with JDK 17+,
the NDK and cargo-ndk (`android\gradlew.bat -p android assembleDebug`); CI is the reference build.
Rule 1 holds there too: Compose never touches the camera, only `LumenSession`.

## Rules that matter

1. **The UI never touches the camera.** All camera I/O happens on the engine thread; the UI sends
   `engine::Command`s and reacts to `engine::Event`s. Keep it that way (COM/WPD objects are thread-bound).
2. **Vendor knowledge stays in `camera/`.** `ptp/` and `link/` stay vendor-neutral; the UI only sees
   `Camera`, `Setting`, `Capabilities`, `LiveImage`.
3. **Every feature must work against the simulator** (`link/sim`) so tests and `LUMEN_SELFTEST` can
   exercise it without a camera. Canon-only behaviour is covered by unit tests on recorded byte layouts.
4. **Hardware safety.** Before opening a real camera, close any running `lumen.exe` (it owns the session).
   Do not fire the shutter or start recording on a real camera unless the user asked for it in this session.
   Always `Camera::close()` (it stops recording, restores SaveTo and leaves remote mode).
5. **Verify UI changes visually**: `LUMEN_SELFTEST=<dir> cargo run --release -p lumen` saves framebuffer PNGs; read them.
   No synthetic mouse/keyboard input against the desktop - it can hit other windows.
6. **File edits on Windows PowerShell 5.1:** `Set-Content`/`Get-Content` default to the ANSI code page and
   corrupt non-ASCII source (`...`, arrows, `x`). Use the Edit tool or `[IO.File]::WriteAllText(path, text, UTF8Encoding($false))`.
7. Style: match the surrounding code; comments explain *why* (protocol quirks, hardware findings), not what.
   No `unwrap()` on I/O paths; errors are `PtpError`/`anyhow` and surface as toasts + activity log.
8. egui is **0.37** (new API): `App::ui`/`logic`, `Panel::left/right/top/bottom(...).show(ui, ..)`,
   `CornerRadius`, `Id::NULL.with(..)`. Do not copy older egui snippets blindly.
9. `ComboBox` inside narrow panels: call `.truncate()` - otherwise it sizes to its longest item and
   silently widens (and shifts) the whole panel.

## Environment variables (all optional, dev only)

| Variable | Effect |
|---|---|
| `LUMEN_SELFTEST=<dir>` | Scripted run of the real UI, screenshots to `<dir>`, exit code 0/1, writes `selftest.ok/.fail`. |
| `LUMEN_SELFTEST_REAL=1` | Use the first real (non-simulated) camera; never takes stills. |
| `LUMEN_SELFTEST_RECORD=1` | With `..._REAL`: also records ~5 s and fails if live view freezes. **Records on the card.** |
| `LUMEN_AUDIO_DEVICE=<name>` | Device for `cargo test probe_audio -- --ignored --nocapture`. |
| `LUMEN_SELFTEST_SHOOT=1` | With `..._REAL`: takes one photo through the app (fires the shutter!) and checks the file + gallery. |
| `LUMEN_SELFTEST_AUDIO=1` | Short run: starts the audio monitor on the first loopback and screenshots the meters (play a tone meanwhile). |
| `LUMEN_PROBE_RECORD=1`, `LUMEN_PROBE_SHOOT=<dir>`, `LUMEN_PROBE_BODYSHOT=<dir>` | Extra scenarios in `probe_real_camera` (record / fire shutter and download / fire like the body button and collect via events). |
| `LUMEN_PROBE_STATE=1`, `LUMEN_PROBE_REINIT=1`, `LUMEN_PROBE_ROUTED=1`, `LUMEN_PROBE_CARDONLY=1` | With `LUMEN_PROBE_RECORD`: print mode properties / run `reinit_remote` / leave SaveTo on card+PC / force card-only. Use them to read the mode dial state. |
| `LUMEN_RAW_FILE=<path>` | `cargo test probe_raw -- --ignored --nocapture` prints the embedded preview of a RAW file. |

## Diagnostic log (read this first when debugging with the user)

The app writes **`logs/lumen.log`** on every run (previous run: `lumen.prev.log`; override with `LUMEN_LOG=<file>`; the
folder is git-ignored). It holds every activity line, one line per raw Canon `GetEvent` record (`prop D1B8 = 4`,
`list D102 count 53`, unknown `record C185 ...`; whole-state dumps are summarised as `initial property dump: N records`),
mode-switch decisions (`mode ...`) and live-view notes (`live ...`). Read it with
`Get-Content logs\lumen.log -Encoding UTF8` (it is UTF-8; plain `Get-Content` shows mojibake).
Camera behaviour is tested on the **real camera only, never the simulator**; physical buttons and the mode dial cannot be
operated by an agent, so ask the user to reproduce and then read the log. The writer is `crates/lumen-core/src/logfile.rs`
(`logfile::line(tag, msg)`).

## Where things were learned the hard way

* Canon status `0x7` on shutter: either the dial is on video (handled: live-view snapshot) **or the PC capacity was not announced (`0x911A`)** - the second was the real photo-mode bug.
* Drive mode may be continuous: force single shot or one release gives several photos (breaks bracketing).
* Shots from the camera body button arrive as `ObjectAddedEx`; `take_new_images` saves them. RAW files also get a JPEG preview (`imaging/raw.rs`).
* Hand changes on the body: the worker polls `GetEvent` every 400 ms, diffs against the settings the UI last saw (`diff_settings`) and emits `Event::CameraChanged` (toast, log, 3 s "changed on camera" tag). Our own writes update `known` first so they are not reported as hand changes; the first 4 s after connecting are ignored (the body is still sending its dump).
* **Mode dial (photo <-> video) is the hardest part; the rules below each cost a bug:**
  * The body does not recover by itself mid-session: record answers "busy" and live view freezes. `Camera::reinit_remote` redoes the remote/event handshake and restarts live view (~3 s). The worker wraps it in `switch_mode`, which emits `Event::ModeSwitching` (full-stage loading overlay) and then `Settings`/`Capabilities`/`LiveView`.
  * Trigger: a whole-state dump (>= 80 records, `Camera::take_big_dump`) that is not ours. Record start/stop dumps are 58-73 records and are ignored (also ignored for 5 s after a recording change and while recording). Cooldown 8 s between switches; a suspicion inside the cooldown stays pending.
  * Reading the mode: **video <=> EVF mode `D1B1 == 2`** (photo is 1); fallback = `D1B8` allowed list contains 4. Do **not** use the `D1B8` value (0 in photo right after opening; 3 in video once live view runs) and do **not** use the allowed list alone (not resent on dial moves).
  * After a dial move the first dump still carries the old state, so `reinit_remote` waits ~1.2 s and re-reads; once live view runs it takes a second opinion from `D1B1`.
  * Do NOT detect the mode from live-view frame kind or from "no frame" stalls: video mode serves JPEG frames while not recording (raw YUV only while recording) and silences are normal. Doing so made the app loop. Stalls are only logged.
  * Video mode needs `SaveTo = card (2)`; photo mode routes to card+PC (6). `ensure_pc_routing` switches both ways and `close()` restores card-only. Leaving PC routing on in video made record answer "busy".
  * `caps.record` (UI enables Record) = Canon in video mode or recording. The HUD shows a PHOTO/VIDEO chip next to LIVE.* Canon live view during recording is **uncompressed UYVY with signed chroma**, not JPEG (`camera/evf.rs`).
* WPD write phase needs both `NUM_BYTES_TO_WRITE` and `TOTAL_DATA_SIZE` or it returns `0x80070490`.
* EOS aperture code is `f = 2^((v-8)/16)`, ISO/shutter step 8 per stop. Tables in `camera/eos.rs`.
* Full list with evidence in `docs/HARDWARE_NOTES.md`.

## Definition of done for a change

`cargo clippy --workspace --all-targets` clean, `cargo test` green, `LUMEN_SELFTEST` passes with the simulator, and for
UI changes you looked at the screenshots. If you touched camera code, run `/probe-camera` when a camera is attached
(ask the user first if it would shoot or record).
