# Roadmap and open questions

Ordered by value. Items marked (?) need a hardware answer before design.

## Verify on the T5i (code exists, never exercised on hardware)
- [x] Photo-mode shutter, download and RAW preview (verified). [ ] Still to confirm physically: pressing the *body* shutter while the
      app runs (verified only by simulating the release), RAW+JPEG quality setting, restore of `SaveTo`/drive mode after a crash.
- [ ] Focus drive in live view (`DriveLens`): step sizes map to 3 speeds only; confirm direction and magnitude.
- [ ] Setting writes for Av/Tv in M mode (`SetDevicePropValueEx`), and which props are writable in video mode.
- [ ] Autofocus trigger for Canon (`DoAf` / `0x9154`?) - currently unsupported (`caps.autofocus == false`).

- [ ] Confirm on the T5i: turning ISO/Av/Tv dials and the mode dial updates the panel (code + unit test exist, not seen on hardware).

## Mode dial and body buttons
- [ ] Verify end to end on the T5i, with the user turning the dial: photo -> video -> photo, loading overlay, PHOTO/VIDEO chip, no bounce (read `logs/lumen.log`).
- [ ] Physical shutter in photo mode with the app connected was reported working; physical record button works in video (`Recording started/stopped` in the log).
- [ ] The overlay hides nothing yet if the dump lands during a capture; consider queuing the switch behind a running bracket.
- [ ] Unknown event record `C185 [01 00 02 00]` arrives about once a second while recording; meaning not identified (maybe time/size left?).

## Video
- [ ] Remaining recording time / file size / resolution (?) - no property identified yet; diff `GetEvent` dumps before/after recording.
- [ ] Mark live-view frame rate drop while recording (~10 fps) in the UI and keep the REC clock independent of frames.
- [ ] Optionally save the live-view feed (UYVY->RGB frames) to a local clip while recording.
- [ ] Movie-mode specific settings (movie size, audio level `D1xx`) via the EOS dump.

## Audio
- [ ] Input selection by stable id rather than display name; remember last device already in `Config`.
- [ ] Per-channel RMS display next to peaks; K-weighted loudness (LUFS) optional.
- [ ] Mic-less machines: document the line-in route from the camera's headphone jack.

## Cameras
- [ ] Nikon on real hardware (ops are from public docs only).
- [ ] Sony / Fujifilm vendor modules behind the same `Camera` surface (`/new-vendor`).
- [ ] libusb path with Zadig-bound WinUSB camera.

## App
- [ ] Persist window geometry; HiDPI audit; light theme.
- [ ] Tethered auto-stack of RAW (needs a RAW decoder - LibRaw/rawler).
- [ ] Sub-pixel alignment before stacking; pyramid/depth-map methods.
- [ ] Packaging: installer/portable zip, app icon, `windows_subsystem` already hides the console in release.
- [x] CI/CD: GitHub Actions (`ci.yml`, `release.yml`), see docs/RELEASING.md. Not yet run on GitHub: first push must be watched.
- [ ] Android v1 follow-ups: bracketing/stacking UI, peaking overlay, MediaStore publishing, hardware test with a real camera over OTG.
