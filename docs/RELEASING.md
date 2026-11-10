# Releasing

CI (`.github/workflows/ci.yml`) runs on every push/PR: clippy + tests on Linux (core, Android crate) and
Windows (whole workspace, release build of the desktop app), and a debug APK build on Linux.

CD (`.github/workflows/release.yml`) runs when you push a tag `vX.Y.Z` (or `vX.Y.Z-rc1`, published as a
pre-release) and attaches to one GitHub Release:

| File | Source |
|---|---|
| `lumen-X.Y.Z-windows-x64.zip` | `cargo build --release -p lumen` |
| `lumen-X.Y.Z-android.apk` | signed, for sideloading (phones and tablets) |
| `lumen-X.Y.Z-android.aab` | signed, for Google Play |
| `SHA256SUMS.txt` | checksums of the above |

The tag is the single source of truth: it is stamped into `Cargo.toml` and into Android `versionName`;
`versionCode = major*10000 + minor*100 + patch`.

## One-time setup: Android signing

```powershell
keytool -genkeypair -v -keystore release.keystore -alias lumen -keyalg RSA -keysize 2048 -validity 10000
[Convert]::ToBase64String([IO.File]::ReadAllBytes("release.keystore")) | Set-Clipboard
```

Create these repository secrets (Settings -> Secrets and variables -> Actions):
`ANDROID_KEYSTORE_B64` (the clipboard content), `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`.
Keep the keystore backed up outside the repo: losing it means existing installs cannot be updated.

## Cutting a release

```powershell
git tag v0.2.0
git push origin v0.2.0
```

`workflow_dispatch` with a version builds all artifacts without publishing, to rehearse a release.

## Building the Android app locally

Requirements: JDK 17, Android SDK + NDK (`ANDROID_NDK_HOME`), `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`,
`cargo install cargo-ndk`. The Gradle wrapper (8.10.2) is committed: use `android\gradlew.bat` (JDK 17 or 21 works).

```powershell
android\gradlew.bat -p android assembleDebug
```

The Gradle build runs `cargo ndk` (libs into `android/app/src/main/jniLibs`) and `uniffi-bindgen`
(Kotlin into `android/app/build/generated/uniffi`); neither output is committed.
