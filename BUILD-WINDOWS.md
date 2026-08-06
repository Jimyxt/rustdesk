# RustDesk Fork v2 — Build & Install Guide for Windows

This fork adds 6 features on top of upstream RustDesk:
1. **Chat persistence** — auto-backup chat + save-as on connection close
2. **Image quality lock** — forces `image_quality = "best"` always
3. **Config defaults** — all permissions enabled by default + adaptive view style
4. **Standalone recording** — record screen (video + local system audio) without a remote connection
5. **Audio recording** — Opus audio track in **all** recordings: `.webm` (VP8/VP9/AV1 via WebmRecorder) and `.mp4` (H264/H265 via the patched hwcodec muxer)
6. **Auto-record on connection** — `allow-auto-record-outgoing` defaults **on**, so an outgoing session starts recording (video + audio) automatically. A guard in `io_loop.rs` skips the auto-start when a standalone recording is already running, avoiding a conflict with the in-progress capture. Users can still toggle it off in Settings.

**Source:** https://github.com/Jimyxt/rustdesk (fork of `rustdesk/rustdesk`)

> ⚠️ **You cannot cross-compile from Linux.** RustDesk uses Windows-only screen capture APIs, vcpkg static libs, and Flutter desktop — all of which require a native Windows toolchain.

---

## Prerequisites

Install these on your Windows 10/11 x64 machine:

| Tool | Version | Download |
|------|---------|----------|
| Visual Studio 2022 | 17.x | https://visualstudio.microsoft.com/ — select **"Desktop development with C++"** workload (MSVC + Windows 10/11 SDK) |
| Rust (stable) | latest | https://rustup.rs — run `rustup default stable-x86_64-pc-windows-msvc` |
| Flutter | 3.x stable | https://docs.flutter.dev/get-started/install/windows |
| vcpkg | latest | https://github.com/microsoft/vcpkg |
| Git | latest | https://git-scm.com/download/win |
| Python 3 | 3.10+ | https://www.python.org/downloads/ — check "Add to PATH" |

**Verify** (open PowerShell and run):
```powershell
rustc --version
flutter doctor
git --version
python --version
```

All should report versions without errors. `flutter doctor` should show green checkmarks (at least for Windows toolchain and VS).

---

## Step 0 — Clone the fork

```powershell
git clone --recursive https://github.com/Jimyxt/rustdesk.git
cd rustdesk
git submodule update --init --recursive
```

> If you already have the repo locally (e.g. copied from the VPS), make sure submodules are initialized:
> ```powershell
> git submodule update --init --recursive
> ```

> ℹ️ This fork vendors a **patched copy of `hwcodec`** under `libs/hwcodec/` (redirected via a `[patch]` in the workspace `Cargo.toml`). The patch adds an **Opus audio track** to the MP4 muxer (`cpp/mux/mux.cpp`) so H264/H265 session recordings include audio instead of being video-only. The upstream git dependency is not used at build time. No action needed — `cargo build` picks up the local fork automatically.

---

## Step 1 — Install vcpkg dependencies

```powershell
# Clone vcpkg (if not already installed)
cd C:\
git clone https://github.com/microsoft/vcpkg
cd vcpkg
.\bootstrap-vcpkg.bat
cd ..

# Set VCPKG_ROOT environment variable (permanent)
$env:VCPKG_ROOT = "C:\vcpkg"
[System.Environment]::SetEnvironmentVariable("VCPKG_ROOT", "C:\vcpkg", "User")

# Install the static libraries RustDesk needs
.\vcpkg\vcpkg install libvpx:x64-windows-static libyuv:x64-windows-static opus:x64-windows-static aom:x64-windows-static
```

> ⏳ This step takes 15–30 minutes. The libraries compile from source.

**Verify:**
```powershell
$env:VCPKG_ROOT = "C:\vcpkg"
ls $env:VCPKG_ROOT\installed\x64-windows-static\lib
```
You should see `.lib` files for vpx, yuv, opus, and aom.

---

## Step 2 — Regenerate Flutter Rust Bridge bindings (CRITICAL)

The fork adds **4 new FFI functions** that don't exist in upstream:
- `save_chat_backup` (chat persistence, v1 feature)
- `main_start_recording` (standalone recording)
- `main_stop_recording` (standalone recording)
- `main_is_standalone_recording` (standalone recording)

Without regenerating the Dart bindings, `flutter build` will fail with "method not found" errors.

```powershell
cd rustdesk

# Install the codegen tool (exact version pinned by RustDesk)
cargo install flutter_rust_bridge_codegen --version 1.80.1 --features uuid --locked

# Install Flutter dependencies
cd flutter
flutter pub get

# Regenerate bindings
$env:USERPROFILE\.cargo\bin\flutter_rust_bridge_codegen --rust-input ../src/flutter_ffi.rs --dart-output ./lib/generated_bridge.dart --c-output ./macos/Runner/bridge_generated.h
cd ..
```

**Verify all 4 bindings were generated:**
```powershell
findstr /C:"saveChatBackup" flutter\lib\generated_bridge.dart
findstr /C:"mainStartRecording" flutter\lib\generated_bridge.dart
findstr /C:"mainStopRecording" flutter\lib\generated_bridge.dart
findstr /C:"mainIsStandaloneRecording" flutter\lib\generated_bridge.dart
```

Each `findstr` should print at least one matching line. If any is missing, the codegen failed — check the error output.

---

## Step 3 — Build

### Option A: Automated build with build.py (recommended)

```powershell
cd rustdesk
$env:VCPKG_ROOT = "C:\vcpkg"

# Full build: Rust lib + Flutter app + portable installer
python build.py --flutter --hwcodec
```

**What this does (in order):**
1. `cargo build --locked --features flutter,hwcodec --lib --release` → produces `target/release/librustdesk.dll`
2. `cargo build --locked --release` in `libs/virtual_display/dylib` → produces `dylib_virtual_display.dll`
3. `flutter build windows --release` → produces `flutter/build/windows/x64/runner/Release/rustdesk.exe`
4. Copies `dylib_virtual_display.dll` into the Flutter build directory
5. Builds the portable packer → produces `rustdesk-<version>-install.exe`

### Option B: Manual build (step-by-step, for debugging)

```powershell
cd rustdesk
$env:VCPKG_ROOT = "C:\vcpkg"

# 1. Build Rust library
cargo build --locked --features flutter,hwcodec --lib --release

# 2. Build virtual display DLL
cd libs\virtual_display\dylib
cargo build --locked --release
cd ..\..\..

# 3. Copy DLL into Flutter build dir (create it first)
mkdir flutter\build\windows\x64\runner\Release -Force
copy target\release\deps\dylib_virtual_display.dll flutter\build\windows\x64\runner\Release\

# 4. Build Flutter Windows app
cd flutter
flutter build windows --release
cd ..

# 5. Copy DLL to final output
copy target\release\deps\dylib_virtual_display.dll flutter\build\windows\x64\runner\Release\
```

### Build output locations

| Artifact | Path |
|----------|------|
| `rustdesk.exe` (Flutter app) | `flutter\build\windows\x64\runner\Release\rustdesk.exe` |
| `librustdesk.dll` (Rust core) | `target\release\librustdesk.dll` |
| `dylib_virtual_display.dll` | `target\release\deps\dylib_virtual_display.dll` |
| Portable installer | `rustdesk-<version>-install.exe` (repo root) |

---

## Step 4 — Install the redistributable

### Option A: Portable installer (recommended)

The `build.py --flutter --hwcodec` command (without `--skip-portable-pack`) automatically generates a portable installer:

```powershell
# The file is in the repo root
.\rustdesk-<version>-install.exe
```

This is a self-extracting portable executable — **no installation required**:
1. Double-click `rustdesk-<version>-install.exe`
2. It extracts to a temp folder and runs RustDesk
3. To "install" permanently: copy the extracted contents to `C:\Program Files\RustDesk\`

### Option B: Manual portable setup

```powershell
# Create install directory
mkdir "C:\Program Files\RustDesk" -Force

# Copy all build artifacts
copy flutter\build\windows\x64\runner\Release\* "C:\Program Files\RustDesk\"

# Create desktop shortcut
$WshShell = New-Object -ComObject WScript.Shell
$Shortcut = $WshShell.CreateShortcut("$env:USERPROFILE\Desktop\RustDesk.lnk")
$Shortcut.TargetPath = "C:\Program Files\RustDesk\rustdesk.exe"
$Shortcut.WorkingDirectory = "C:\Program Files\RustDesk"
$Shortcut.Save()

# Create Start Menu shortcut
$Shortcut2 = $WshShell.CreateShortcut("$env:APPDATA\Microsoft\Windows\Start Menu\Programs\RustDesk.lnk")
$Shortcut2.TargetPath = "C:\Program Files\RustDesk\rustdesk.exe"
$Shortcut2.WorkingDirectory = "C:\Program Files\RustDesk"
$Shortcut2.Save()
```

### Option C: Run directly from build folder (for testing)

```powershell
cd flutter\build\windows\x64\runner\Release
.\rustdesk.exe
```

> ⚠️ The `dylib_virtual_display.dll` **must** be in the same folder as `rustdesk.exe`. If you copy the exe elsewhere, copy the DLL too.

### Post-install: enable startup (optional)

To make RustDesk start automatically with Windows:

```powershell
# Add to startup
$WshShell = New-Object -ComObject WScript.Shell
$Shortcut = $WshShell.CreateShortcut("$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Startup\RustDesk.lnk")
$Shortcut.TargetPath = "C:\Program Files\RustDesk\rustdesk.exe"
$Shortcut.Save()

# Or via registry
reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v RustDesk /t REG_SZ /d "C:\Program Files\RustDesk\rustdesk.exe" /f
```

---

## Step 5 — Verify the features work

### Feature 1 — Config Defaults
1. Launch `rustdesk.exe`
2. Open **Settings** → all permission toggles should be **enabled by default**:
   - Keyboard, Clipboard, File Transfer, Audio, Terminal, TCP Tunnel, Remote Restart, Record Session, Block Input, Privacy Mode, Remote Config Modification
3. **View Style** should be set to **"Adaptive"**
4. **Image Quality** should be **"Best"** and greyed out (locked)

### Feature 2 — Standalone Recording
1. On the main screen (no remote connection), look for the **record button** near the settings menu
2. Click it — the icon should turn **red** (recording active)
3. Wait a few seconds, click again to **stop**
4. Check your Videos folder (`%USERPROFILE%\Videos\`) — a `.webm` file should appear
5. Play the file — it should show your screen recording **with audio** (local system audio captured via WASAPI loopback on Windows, ScreenCaptureKit on macOS, PulseAudio monitor on Linux) as an Opus track
6. Verify with `ffprobe <file>.webm` — it should list both `Video: vp9` and `Audio: opus, 48000 Hz, stereo` with a non-zero packet count

### Feature 3 — Audio Recording (remote sessions)
1. Connect to a remote peer (or have someone connect to you)
2. Start a session recording (the existing record button in the remote toolbar)
3. Exchange some audio (talk on both ends)
4. End the session — check the recording file
5. Play the recording — it should have **audio from the remote peer** (what you hear through the connection)
6. Audio works for **all** codecs: VP8/VP9/AV1 produce a `.webm` with an Opus track (WebmRecorder); H264/H265 (hwcodec) produce a `.mp4` with an Opus track (patched hwcodec muxer). Verify with:
   - `ffprobe <file>.webm` → `Audio: opus, 48000 Hz, stereo`
   - `ffprobe <file>.mp4` → `Audio: opus, 48000 Hz, stereo`
7. ⚠️ **Scope:** Session recordings capture the **remote peer's audio** (what you hear). Local system audio on the recording side is captured by the standalone recorder (Feature 2), not the session recorder.

### Feature 4 — Chat Persistence
1. Connect to a peer, open chat, exchange messages
2. Close the connection
3. A "Save as" dialog appears — choose a location or cancel
4. Check `%APPDATA%\rustDesk\chat_backup\` — a `.txt` file should exist with the chat log

### Feature 5 — Image Quality Lock
1. Open Settings → Image Quality
2. Radio buttons (Best / Balanced / Low / Custom) should be **disabled/greyed out**
3. In-session toolbar quality menu is visually editable but clicking "Low" does nothing (Rust-side guard no-ops)

### Feature 6 — Auto-record on connection
1. With **no** standalone recording running, connect to a remote peer
2. The session should start recording automatically (record indicator active); a recording file (`.webm` for VP8/VP9/AV1, `.mp4` for H264/H265) appears with both video and audio
3. **Conflict guard:** start a standalone recording first (Feature 4), then connect to a remote peer — the auto-record should be **skipped** (log: `auto-record on connection skipped: standalone recording in progress`), and only the standalone recording continues. Stop the standalone recording and reconnect to confirm auto-record resumes.
4. To opt out: Settings → Recording → uncheck "Automatically record outgoing sessions" (sets `allow-auto-record-outgoing` = `N`)

---

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `cargo build` fails with vcpkg errors | Ensure `VCPKG_ROOT` env var is set: `$env:VCPKG_ROOT = "C:\vcpkg"`. Verify packages installed in Step 1. |
| `bind.mainStartRecording` not found | You skipped Step 2 (codegen). Run `flutter_rust_bridge_codegen` again. |
| `bind.saveChatBackup` not found | Same — run codegen (Step 2). This is a v1 feature binding. |
| `flutter build` fails after codegen | Run `flutter clean && flutter pub get` then rebuild. |
| `dylib_virtual_display.dll` not found | Build it separately: `cd libs\virtual_display\dylib && cargo build --release`. Copy to the same folder as `rustdesk.exe`. |
| `cargo build` — `chrono` not found | Build from repo root (not `flutter/`): `cargo build --locked --features flutter --lib --release` |
| Settings toggles not defaulted | Defaults apply after `read_custom_client()` runs at init — restart the app. |
| Recording has no audio | Verify with `ffprobe <file>` that no audio stream is present, then check the codec: VP8/VP9/AV1 use WebmRecorder; H264/H265 use the patched hwcodec MP4 muxer. If the `.mp4` lacks audio, confirm the build used the vendored `libs/hwcodec` fork (the `[patch]` in `Cargo.toml` must not be removed). On Windows the Opus-in-MP4 path requires a recent FFmpeg via vcpkg. |
| Standalone recording has no audio | The standalone recorder captures local system audio (WASAPI loopback) via `src/standalone_audio.rs`. If absent, check that the default audio output device is reachable and not muted; on Linux ensure a PulseAudio monitor source exists. |
| Build is slow (>30 min) | Normal for first build. Rust compiles ~300 crates. Subsequent builds use caching and are faster. |
| Portable packer fails | Use `--skip-portable-pack` flag: `python build.py --flutter --hwcodec --skip-portable-pack`. Use Option B or C for installation. |

---

## Quick reference — full build from scratch

```powershell
# 1. Clone
git clone --recursive https://github.com/Jimyxt/rustdesk.git
cd rustdesk
git submodule update --init --recursive

# 2. Set vcpkg
$env:VCPKG_ROOT = "C:\vcpkg"

# 3. Codegen
cargo install flutter_rust_bridge_codegen --version 1.80.1 --features uuid --locked
cd flutter
flutter pub get
$env:USERPROFILE\.cargo\bin\flutter_rust_bridge_codegen --rust-input ../src/flutter_ffi.rs --dart-output ./lib/generated_bridge.dart --c-output ./macos/Runner/bridge_generated.h
cd ..

# 4. Build (Rust + Flutter + portable installer)
python build.py --flutter --hwcodec

# 5. Run
.\rustdesk-<version>-install.exe
# or
flutter\build\windows\x64\runner\Release\rustdesk.exe
```

---

## Rollback (if needed)

```powershell
cd rustdesk
git checkout master -- .
git clean -fd src/standalone_recorder.rs 2>$null
Remove-Item src/standalone_recorder.rs -ErrorAction SilentlyContinue

# Regenerate bindings without the new FFI functions
cd flutter
flutter_rust_bridge_codegen --rust-input ../src/flutter_ffi.rs --dart-output ./lib/generated_bridge.dart --c-output ./macos/Runner/bridge_generated.h
```