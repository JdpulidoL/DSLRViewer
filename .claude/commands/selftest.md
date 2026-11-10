---
description: Run the scripted GUI self-test against the simulator and review the screenshots
argument-hint: "[real] [record]"
---
Goal: prove the real UI still works end to end and look at it.

1. `cargo build --release`.
2. Pick a scratch dir (the session scratchpad). Run `lumen.exe` with `LUMEN_SELFTEST=<dir>` and wait for it to exit
   (timeout ~200 s). Exit 0 and `<dir>\selftest.ok` mean pass; `selftest.fail` holds the reason.
3. Read `01-live.png`, `03-stacked.png`, `05-fullscreen.png` (and the others if relevant) and check layout, overlaps,
   clipped text, wrong colours. Describe real visual defects and fix them.

Arguments (`$ARGUMENTS`): `real` adds `LUMEN_SELFTEST_REAL=1` (needs a camera; close any running lumen.exe first; never takes stills);
`record` additionally sets `LUMEN_SELFTEST_RECORD=1` - this records ~5 s on the camera card, so ask the user before using it.
See docs/TESTING.md.