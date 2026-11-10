---
description: Run the full quality gate (clippy, tests) and report only what fails
---
Run, in `lumen/`, with `$env:Path += ";$env:USERPROFILE\.cargo\bin"`:

1. `cargo clippy --all-targets` - must produce no warnings.
2. `cargo test` - all tests must pass (ignored hardware probes are expected).

Report failures with file:line and fix them. Do not touch real hardware. If everything passes, say so in one line.