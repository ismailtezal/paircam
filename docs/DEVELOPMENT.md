# Development

Everything here runs on Windows 11. The iPhone app is built in the cloud (see `apps/mobile`).

## One-time setup

- Visual Studio 2022 or newer with the "Desktop development with C++" workload and a Windows 11
  SDK.
- Rust through [rustup](https://rustup.rs). The repository pins the MSVC toolchain.
- Node.js 20 or newer (for the design token generator and the phone app).
- FFmpeg, which the engine links against:

  ```bash
  powershell -ExecutionPolicy Bypass -File scripts/fetch-ffmpeg.ps1
  ```

  This downloads a pinned, checksum-verified LGPL build into `third_party/ffmpeg` (ignored by
  git).

## Running the desktop app

```bash
cargo run -p paircam-desktop
```

Useful options: `--connect <address>` connects at startup, `--locale <en|tr>` overrides the
language, `--light` starts in the light theme, `--settings` opens the settings sheet, and
`--data <folder>` keeps settings somewhere other than `%APPDATA%\PairCam`.

## Working without an iPhone

The simulator is a fake phone. It advertises itself on the network, pairs, and streams a moving
test pattern that FFmpeg encodes in real time on the GPU:

```bash
cargo run -p paircam-engine --example simulator
```

The desktop app finds it under "Phones" and chooses the size, frame rate and codec, as it does
with a real phone. Options: `--encoder <name>` picks another FFmpeg encoder when there is no
NVIDIA GPU (for example `h264_mf`), `--decline` refuses pairing, and `--pair-delay <ms>` changes
how long the simulated user takes to accept.

To check the receiving pipeline from a terminal, without the interface:

```bash
cargo run -p paircam-engine --example headless -- 127.0.0.1:47810 --seconds 6 --save frame.bmp
```

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace
node design/tokens/build.mjs --check
```

## Design tokens

Edit `design/tokens/tokens.json`, then regenerate the desktop theme, the Rust constants and the
phone theme:

```bash
node design/tokens/build.mjs
```

Never edit the generated files (`crates/ui/assets/themes/paircam.json`,
`crates/ui/src/tokens.rs`, `apps/mobile/src/theme/tokens.ts`).

## Protocol changes

Change `docs/PROTOCOL.md`, `crates/protocol` and the Swift implementation together, then
regenerate the shared test vectors:

```bash
PAIRCAM_UPDATE_VECTORS=1 cargo test -p paircam-protocol --test vectors
```

## Looking at the interface without a person at the screen

`scripts/window-shot.ps1` saves a PNG of the running app's window, even when it is behind other
windows:

```bash
powershell -ExecutionPolicy Bypass -File scripts/window-shot.ps1 -Process paircam -Out shot.png
```

## The virtual camera

`native/vcam` is the DLL Windows loads when an app opens the "PairCam" camera. Build it and run
its test harness, which drives the DLL without registering it:

```bash
powershell -ExecutionPolicy Bypass -File native/vcam/build.ps1 -Test
```

Installing it needs administrator rights once, because Windows only loads camera components
registered for the whole computer. The desktop app offers "Install camera", which runs:

```bash
powershell -ExecutionPolicy Bypass -File scripts/register-vcam.ps1
```

Run it again after rebuilding the DLL. `scripts/unregister-vcam.ps1` removes it. The camera is
visible to other apps only while PairCam is running.
