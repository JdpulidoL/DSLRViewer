# Hardware notes (verified on a Canon EOS Rebel T5i / 700D, Windows 11, WPD)

Everything marked **verified** was observed on the real body through this app's own probes
(`cargo test probe_real -- --ignored --nocapture`, `LUMEN_SELFTEST_REAL`). Everything marked
*assumed* comes from public PTP/EOS knowledge and has not been seen on hardware.

## Getting at the camera on Windows

* The T5i binds to the stock Windows driver and shows up in **WPD** (`vid_04a9&pid_3272`). libusb cannot open it
  ("Access denied / insufficient permissions") unless the interface is re-bound to WinUSB - so `link::scan` hides the
  libusb twin of any device WPD already lists. **verified**
* Its WPD functional categories contain only *storage*, not *still image capture*, so cameras are recognised by
  category **or** by maker name (`wpd::looks_like_camera`). A USB hard drive also appears in WPD and must not be listed. **verified**
* WPD tunnels PTP through "MTP extension" commands. Read phase: `EXECUTE_COMMAND_WITH_DATA_TO_READ` -> `READ_DATA`
  chunks -> `END_DATA_TRANSFER`. Write phase needs **both** `TRANSFER_NUM_BYTES_TO_WRITE` and `TRANSFER_TOTAL_DATA_SIZE`
  or the driver answers `0x80070490`. Empty parameter collections must still be sent for read-phase ops. **verified**
* The driver owns the PTP session (`manages_session() == true`); `GetDeviceInfo` works through the extension. **verified**
* No PTP interrupt events through WPD: new objects are found by listing handles; Canon uses `GetEvent` polling anyway. **verified**
* Only one process can talk to the camera. A running `lumen.exe` (or EOS Utility) blocks probes: events come back empty
  and property writes fail. Close it first.

## Canon EOS session

* Vendor extension id is **6** (Microsoft) with an empty description, not 0x0B. Detect Canon by maker name + presence
  of ops `0x9114` (SetRemoteMode) and `0x9116` (GetEvent). **verified**
* Open: `SetRemoteMode(1)`, `SetEventMode(1)`, then `GetEvent` returns the full property dump once per session.
  Close: `SetEventMode(0)`, `SetRemoteMode(0)`; without this the next session may get no dump (empty settings). **verified**
* `GetEvent` payload = records `[size u32][type u32]...`; `0xC189` property value `(code, value)`, `0xC18A` allowed list
  `(code, datatype, count, values...)` - element width is derived from record size, `0xC181` object added. **verified**
* Property codes used (`camera/eos.rs`): Av `D101`, Tv `D102`, ISO `D103`, EC `D104`, AE mode `D105`, metering `D107`,
  focus mode `D108`, WB `D109`, SaveTo `D11C` (allowed `[2 card, 4 PC, 6 both]`), EVF output `D1B0`, EVF mode `D1B1`,
  record status `D1B8`. **verified present**; labels for ISO/Tv/Av/EC follow the formulas below.
* Value formulas: ISO `100 * 2^((v-0x48)/8)`; shutter `30 s * 2^(-(v-0x10)/8)`; **aperture `2^((v-8)/16)`** (an earlier
  `/8` was wrong and showed f/13 for f/3.5); EC is a signed byte in 1/8 EV. Nominal snapping lists live in `eos.rs`.
* Writes use `SetDevicePropValueEx 0x9110` with payload `[12][prop][value]`. It answers `DeviceBusy 0x2019` while the
  body is mid-change (retry with `canon_set_prop_retry`). **verified**
* AE mode reads `Manual` in both photo and video dial positions, so it cannot tell them apart. See *Mode dial* below.
* Hand changes on the body (dials, menus) arrive as `0xC189` property events and are polled every 400 ms. **verified** (log of a session)

## Live view

* Start: output device `D1B0` needs bit 2 (PC). In video mode the body already uses EVF mode `D1B1 = 2` and **refuses to be
  switched to 1** (`DeviceBusy`) - only write `D1B1` if it is 0. **verified**
* `GetViewFinderData 0x9153 [0x100000]`; `NotReady 0xA102` just means no frame yet (treated as transient). **verified**
* Photo mode payload contains a JPEG (960x640). **Video mode payload is not JPEG**: a header record (`size 0x2c`,
  type `0xFFFFFFFF`) followed by a type-9 record: `size, 9, flags, w, h, 0, 0, w, h, fmt` (40 bytes) then `w*h*2` bytes of
  **UYVY**, luma full range, **chroma bytes are signed (two's complement, centred on 0)**, then ~4 KiB of metadata.
  Typical 720x480, 695,552 bytes total. Decoding it as JPEG produced the "Not enough bytes" toasts and a frozen preview. **verified** (decoded to a correct colour image)
* While recording the body flips `D1B0` to 3 (screen + PC) and still serves frames (~10 fps, every other call `NotReady`). **verified**
  An earlier theory that frames stop during recording was wrong - the JPEG parser was simply failing on the YUV record.

## Video recording

* Write `D1B8 = 4` to start, `0` to stop; confirm by the next property event (poll up to ~2.5 s). **verified** (dial on video, card inserted)
* With the dial **not** on video the same write answers `DeviceBusy`. The error message says to turn the dial.
* `close()` stops a running recording before leaving remote mode.

## Still capture (photo-mode shutter: verified)

* **Before the shutter, tell the body how much room the PC has**: `PC_HDD_CAPACITY 0x911A [0x0FFFFFFF, 0x1000, 1]`
  whenever SaveTo includes the PC (4 or 6). Without it `RemoteRelease` answers status **0x7** even in a still mode.
  This was the real cause of the persistent "refused to take the picture" error. **verified** (CR2 of ~22.7 MB downloaded)
* The sequence that works: SaveTo -> 6 (card+PC), announce capacity, force **single-shot drive**, `RemoteRelease 0x910F`
  (status 0 = ok), poll `GetEvent` for `ObjectAddedEx 0xC181` (handle + file name inside the record), `GetPartialObject 0x9107`
  in 1 MiB chunks, `TransferComplete 0x9117`. **verified**
* **Drive mode `D106`**: the body was in continuous drive (value 1), so one release delivered two photos (IMG_0643 + 0644)
  while live view was on. `force_single_shot` sets 0 and `close()` restores the user's mode. **verified**
* Pictures taken with the **camera's own shutter button** raise the same `ObjectAddedEx` events. They are collected by
  `poll_settings` -> `pending_added` and downloaded by `take_new_images` (worker `collect_body_shots`) into the configured
  output folder. Requires still mode + PC routing (`ensure_pc_routing`, done at connect and re-done after leaving video mode).
  **verified at camera level** (`LUMEN_PROBE_BODYSHOT`), not by physically pressing the button.
* The T5i here was set to **RAW only** (`.CR2`, ~22 MB). The app also writes the embedded full-size preview (`imaging/raw.rs`,
  5184x3456 JPEG, found through the TIFF IFDs; the sensor data is a *lossless* SOF3 JPEG and is skipped) next to it so it
  can be shown and stacked. **verified**
* Old notes kept for history: status 0x7 was first blamed on the dial being on video (true for video mode, where the app
  now saves a live-view snapshot) - but in a still mode the missing capacity announcement was the cause.

## Still capture (earlier notes)

* `RemoteRelease 0x910F` returns status word 0 on success. **Status `0x7` = stills unavailable (dial on video)**; same with
  half/full press `0x9128` pairs. The app therefore saves a live-view snapshot in video mode. **verified** that 0x7 occurs; the
  photo-mode shutter path (destination switch to card+PC, `ObjectAddedEx`, `GetPartialObject`) is **implemented but not yet
  exercised on hardware** - test it with the dial on a still mode (`LUMEN_PROBE_SHOOT=<dir>`; ask the user, it fires the shutter).
* RAW+JPEG announces two objects; the worker records the viewable one and bracketing only stacks decodable formats.

## Mode dial (photo <-> video)

Measured with `LUMEN_PROBE_STATE` and the diagnostic log (`logs/lumen.log`); all **verified** on the T5i.

| State | `D1B8` record status | `D1B1` EVF mode | `D1B8` allowed list |
|---|---|---|---|
| Photo, session just opened | **0** | 1 (list 1,0) | empty |
| Photo, live view running | 3 | 1 | empty |
| Video, session just opened | 0 | **2** | 4,0 |
| Video, live view running | **3** | 2 | 4,0 |
| Recording | 4 | 2 | 4,0 |

* So the record status alone is ambiguous (0 in photo, 3 in video). The reliable discriminator is **`D1B1`: 2 = video, 1 = photo**;
  the allowed list (contains 4 only in video) is a fallback but is **not resent when the dial moves**.
* Turning the dial mid-session makes the body send a **whole-state dump** (87-129 records) and take the viewfinder away
  (`D1B0` falls back to 1/0). Without action, record answers `DeviceBusy` and live view stays silent for 30+ s.
  Record start/stop also dump, but only 58-73 records.
* Fix: redo `SetEventMode(0)/SetRemoteMode(0)` -> `SetRemoteMode(1)/SetEventMode(1)`, wait ~1.2 s for the dump to settle
  (the first dump still holds the old state), read the mode, restart live view (`Camera::reinit_remote`, ~3 s). **verified**
  on video and photo; the physical dial turn itself was only verified through the logs of the user's sessions.
* Video needs `SaveTo = 2` (card). Leaving `SaveTo = 6` (card+PC) from photo mode made `D1B8 = 4` answer `DeviceBusy`,
  and the body's own record button was blocked too. **verified**
* The body re-enumerates in WPD after power cycles (new device id, `0x80070002` on open); the worker rescans and retries.
* A card that is not recognised makes record busy as well; a reformat in the camera fixed it once.
* Live view frame kinds: photo = JPEG 960x640; video = JPEG while idle and raw UYVY 720x480 only while recording
  (~10-12 fps). Frame kind and silences are **not** usable as mode signals.

## Other facts

* Battery: standard property `0x5001` via `GetDevicePropDesc` works over EOS (values are coarse: 100/50/...). Refreshed every 10 s.
* The T5i auto-powers-off / disappears from WPD after inactivity or after heavy probing; unplug/replug or power-cycle it.
* `ComboBox` without `.truncate()` widens its panel to its longest item (UI quirk, not hardware).

## Audio

* No audio over PTP. The PC reads a microphone/line-in/capture card, or **WASAPI loopback** of an output device.
  Verified with a 440 Hz tone through loopback: peak ~ -11.7 dBFS and `Started{channels: 2, 48 kHz}`.
  This machine had no microphone attached when tested.

## Android (Canon EOS Rebel T5i, Infinix tablet, Android 15, USB host)

* libusb over `UsbDeviceConnection.getFileDescriptor()` (`libusb_wrap_sys_device`) claims the interface but **every bulk read
  times out**, even right after a successful write. **verified** (OpenSession never answered; a device reset made it worse).
* The same bytes through `UsbDeviceConnection.bulkTransfer` work at once: OpenSession -> `0x2001`, GetDeviceInfo -> 547 bytes. **verified**
  So the Android build does the USB I/O in Kotlin and hands PTP containers to Rust (`UsbTransport` / `link::register_bridge`).
* `bulkTransfer` returns -1 for both timeouts and unplugging; Kotlin tells them apart by checking `UsbManager.deviceList`.
* Reinstalling the app drops its USB permission, so the first tap after an install shows the system dialog again.
* The camera re-enumerates (new `/dev/bus/usb/001/NNN`) when it is power-cycled or after a failed session; scan again after that.
