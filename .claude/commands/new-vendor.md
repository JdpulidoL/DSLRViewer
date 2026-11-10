---
description: Checklist for adding support for another camera vendor
argument-hint: "<vendor name>"
---
Add `$ARGUMENTS` support without changing `ptp/` or the UI:

1. Identify it: extend `camera::detect_vendor` (extension id, maker name, signature operations) and add a `Vendor` variant.
2. Capabilities: fill `Capabilities` in `Camera::open` from `DeviceInfo.operations`.
3. Implement per-vendor branches in `camera/mod.rs` (live view, focus drive, capture, settings). If the vendor has its own
   property model, give it a module like `camera/eos.rs` that produces `Vec<Setting>`.
4. Live-view payload parsing goes in `camera/evf.rs` with a unit test on a recorded byte layout.
5. If the WPD driver needs vendor read-phase ops, add them to `link::wpd::returns_data`.
6. Extend the simulator only if needed for engine tests; otherwise unit-test byte layouts.
7. Document in docs/HARDWARE_NOTES.md what is verified vs assumed. Run `/check` and `/probe-camera`.