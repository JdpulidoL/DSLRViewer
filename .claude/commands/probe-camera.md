---
description: Probe the attached camera (read-only unless the user agrees to shoot/record)
argument-hint: "[record] [shoot <dir>]"
---
1. Close any running lumen.exe, then verify the camera: `Get-PnpDevice -PresentOnly | ? InstanceId -match 'VID_04A9'`
   (Canon) - if it is missing, tell the user to power-cycle/replug it and stop.
2. `cargo test probe_real -- --ignored --nocapture` prints identity, capabilities, the Canon EOS property dump
   (`eos 0x....: (value, [allowed])`), a live-view frame check and the settings list.
3. Only with the user's explicit OK in this session: `LUMEN_PROBE_RECORD=1` (records ~6 s) or `LUMEN_PROBE_SHOOT=<dir>` (fires the shutter).
4. Compare against docs/HARDWARE_NOTES.md. Add new facts there (with "verified"/"assumed") and as unit tests on byte layouts.