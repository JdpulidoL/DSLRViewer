# Architecture

```
                    +---------------------------- app/ (egui, main thread) ---------------------------+
                    |  LumenApp state  <--- Event ---+                +--- audio::Notice / Levels ---+ |
                    |  panels/widgets  --- Command -+                |                              | |
                    +-------------------------------|----------------|------------------------------+
                                                    v                v
                         engine/worker.rs (thread "camera-engine")   audio/ (thread "audio-monitor", cpal)
                                  |  owns
                                  v
                            camera::Camera  (vendor logic: Nikon / Canon EOS / generic)
                                  |  uses
                                  v
                          ptp::Session -> dyn PtpTransport
                                  |
          +-----------------------+---------------------------+
          v                       v                           v
  link::wpd::WpdTransport   ContainerTransport<UsbLink>   ContainerTransport<SimLink>
  (Windows MTP/PTP driver)  (libusb, WinUSB cameras)      (virtual DSLR, real wire format)
```

## Threads and ownership

* **Main thread** - egui. Never blocks on I/O. Calls `ctx.request_repaint()` through a `wake` closure
  that the worker threads invoke after queuing an event.
* **camera-engine** - single owner of `Option<Camera>`. Loop: take a `Command` (or time out), service live
  view (`pump_live_frame`, ~30 fps cap), poll the camera for self-made changes (`poll_camera_settings`, 700 ms),
  report recording state changes. Long operations (bracket) keep reading the command channel for `CancelBracket`.
  Stacking runs on a separate short-lived thread so the camera keeps streaming.
* **audio-monitor** - owns cpal streams (not `Send`). UI reads `Levels` (atomics) every frame.
* On Windows the camera thread initialises COM (MTA) via `link::wpd::ComGuard`; `WpdTransport` is
  `unsafe impl Send` on that basis. Create and use it on the engine thread only.

## PTP layers

* `ptp::codec` - containers (`Command/Data/Response/Event`), little-endian reader/writer, PTP strings.
* `ptp::dataset` - `DeviceInfo`, `PropDesc`, `Value` (+ encode/decode so the simulator can reuse them).
* `ptp::transport` - `PtpTransport::transact(op, params, data_out) -> Reply` and `ContainerTransport`
  which frames/reassembles over any `WireLink` (bulk chunks, transaction ids, OpenSession tid 0).
* `ptp::Session` - open/close, `call` (expects OK), property helpers, event polling, object handles.

## Camera layer (`camera/`)

`Camera::open(transport)` detects the vendor (`detect_vendor`: extension id **or** maker name + signature ops,
because Canon reports extension 6) and builds `Capabilities { live_view, focus_drive, autofocus, record }`.

| Concern | Nikon / generic | Canon EOS |
|---|---|---|
| Settings | standard property descriptors (`settings.rs`) | event stream `GetEvent 0x9116` folded by `eos::EosState`, values decoded by tables |
| Live view | `0x9201/0x9202/0x9203`, JPEG scanned out of payload | EVF props `0xD1B0/0xD1B1`, `GetViewFinderData 0x9153`; JPEG **or** UYVY record (`evf.rs`) |
| Focus drive | `MfDrive 0x9204` | `DriveLens 0x9155` (3 speeds) |
| Capture | `InitiateCapture` + `ObjectAdded` event (or polling handles on WPD) | `RemoteRelease 0x910F`, `ObjectAddedEx` events, `GetPartialObject 0x9107`; destination switched to card+PC and restored on close |
| Video | - | `EVFRecordStatus 0xD1B8` write 4/0, state from events |

`LiveImage` is `Jpeg(bytes)` or `Rgb{w,h,pixels}`; `imaging::decode_live` turns either into a `Frame`
(RGBA + histogram + sharpness + optional peaking).

## Engine protocol (`engine/mod.rs`)

`Command`: `Scan, Connect, Disconnect, SetSetting, LiveView, Record, SetPeaking, Capture, Focus, Autofocus,
Bracket, CancelBracket, Stack, Shutdown`.
`Event`: `Devices, Connected, Disconnected, Settings, LiveView, Recording, Frame, Captured, BracketProgress,
BracketFinished, StackProgress, StackFinished, StackFailed, Log`.
Adding a feature = add a variant on each side, handle it in `worker::handle`, apply it in `LumenApp::apply_event`.

Video-mode stills: when `Camera::video_mode()` the worker saves the last live-view frame as the "capture"
(`live_snapshot`) because the body refuses real stills in that mode.

## UI (`app/`)

* `mod.rs` - `LumenApp` state, `drain_events` (coalesces frames, uploads only the newest texture),
  shortcuts, layout (`Panel`s + `CentralPanel`; fullscreen swaps to `viewer::show_fullscreen`).
* `viewer.rs` - stage drawing: image + zoom/pan, grid, histogram, sharpness meter, **camera OSD**
  (`osd`, `exposure_scale`), audio meters overlay, REC border, control bar, fullscreen toolbar.
* `camera_panel.rs`, `tools_panel.rs` (focus, bracket, stack, audio card, output), `dock.rs` (captures,
  activity, toasts), `header.rs`, `widgets.rs` (all custom-painted widgets + icons), `theme.rs`, `meters.rs`.
* `selftest.rs` - state machine driven from `logic()`; see `docs/TESTING.md`.
* Persisted prefs: `config.rs` -> `%APPDATA%\Lumen\config.json` (serde, `#[serde(default)]`).

## Simulator (`link/sim`)

`SimCamera` implements a Nikon-style op set on the real wire format (`SimLink` chunks bulk reads to 4 KiB to
exercise reassembly). `scene.rs` renders a layered scene whose blur depends on focus position and aperture,
brightness on ISO/shutter/aperture/EV, colour on WB; capture renders a 1800x1200 JPEG. It does **not** emulate
Canon EOS or video recording - those are verified with byte-level unit tests and the hardware probes.

## Stacking (`imaging/stack.rs`)

Per-frame focus map = box-blurred absolute Laplacian of luma. `Weighted`: blend by `measure^4`.
`Sharpest`: per-pixel argmax, one-hot masks feathered by box blur, normalised blend. No alignment.

## Audio (`audio/`)

PTP cameras carry no audio over USB. The monitor reads any cpal input (microphone, line-in, capture card) and,
on Windows, **loopback** of an output device (`Loopback: <name>`, "system sound"). Peak/RMS per channel are
published lock-free; UI applies ballistics (`app/meters.rs`). "Listen" plays the mono mix to the default output
at the input's sample rate through a bounded queue (~0.4 s max latency).

## Diagnostics and mode handling (added later)

* `src/logfile.rs`: plain-text log at `logs/lumen.log` (rotated to `lumen.prev.log`), written by the worker's `log` and by
  `Camera::poll_eos` (one line per raw `GetEvent` record). It is the primary debugging channel with the real camera.
* Extra engine events: `CameraChanged(Vec<Change>)` (hand changes on the body), `Capabilities(Capabilities)` (record available
  or not), `ModeSwitching(Option<bool>)` (loading overlay while the camera re-initialises after a dial move).
* Worker state added for this: `known` (settings last shown, `RefCell` so `with_camera` closures can update it),
  `caps`, `connected_at`/`SETTLE_AFTER_CONNECT`, `last_switch`/`SWITCH_COOLDOWN`, `rec_changed_at`, `want_live`
  (live view is re-started quietly if the body drops it), `pending_check`.
* Camera state added: `mode_video` (mode as judged from a handshake), `big_dump` flag (`take_big_dump`), `in_video()` which
  `caps.record`, `ensure_pc_routing` and `take_new_images` consult.
* UI: `mode_switch` drives `mode_switch_overlay` (viewer.rs); `hand_changed` drives the "changed on camera" tag; the HUD
  shows a PHOTO/VIDEO chip. The dev self-test renders the overlay as `06-mode-switch.png`.
* Live-view pacing: 12 ms frame interval, 4 ms retry after "no frame yet" (the body answers every other call while recording).
