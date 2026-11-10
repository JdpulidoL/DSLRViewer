# Testing

| Layer | How | Needs hardware |
|---|---|---|
| Codecs, datasets, EOS tables/events, UYVY decode, stacking, meters, config | `cargo test` (unit tests next to the code) | no |
| Engine + camera + PTP wire format end to end | `src/e2e.rs` against `link::sim` (connect, live view, settings, capture, bracket, cancel, unplug) | no |
| Whole GUI | `LUMEN_SELFTEST=<dir>`: connects, changes ISO, defocus/AF, capture, 8-shot bracket, stack, activity, fullscreen; starts the audio monitor; saves PNGs | no |
| Real camera, read-only | `LUMEN_SELFTEST=<dir> LUMEN_SELFTEST_REAL=1` (live view + OSD + fullscreen, no stills) | yes |
| Real camera, records | add `LUMEN_SELFTEST_RECORD=1` - records ~5 s on the card and fails if fewer than 15 live frames arrive | yes (card) |
| Raw camera probe | `cargo test probe_real -- --ignored --nocapture` (settings, EOS property dump, live view); `LUMEN_PROBE_RECORD=1` runs a 14 s record scenario with per-second frame counts; `LUMEN_PROBE_SHOOT=<dir>` fires the shutter and saves what is downloaded | yes |
| Audio | `cargo test probe_audio -- --ignored --nocapture` (set `LUMEN_AUDIO_DEVICE='Loopback: <output name>'` and play a sound) | an input or any output |

## Procedure for hardware work

1. Close any running `lumen.exe` (`Get-Process lumen | % CloseMainWindow`), confirm the camera is present
   (`Get-PnpDevice -PresentOnly | ? InstanceId -match 'VID_04A9'`).
2. Ask before anything that moves the mirror, fires the shutter or records.
3. Capture raw payloads when a format is unknown, write them to a scratch dir, and render/inspect them instead of guessing
   (this is how the UYVY format was found: dump -> render candidate pixel layouts -> look at the PNG).
4. Turn findings into a unit test on the byte layout (`camera/evf.rs` tests are the template) and record them in `HARDWARE_NOTES.md`.

## Reading self-test output

`<dir>/selftest.ok` or `selftest.fail` (reason inside), `01-live.png`, `02-*.png`, `03-stacked.png`, `04-activity.png`,
`05-fullscreen.png`, `captures/`. Self-test stage numbers are in `app/selftest.rs`; `st.stage = N` followed by a screenshot
lands on **N+1** because the screenshot step calls `advance()`.
