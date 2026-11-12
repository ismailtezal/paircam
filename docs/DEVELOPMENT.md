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

The simulator also plays a person for face tracking and hand gestures. An orange square stands
in for the face: it wanders around the picture and leaves for a few seconds now and then. While
face tracking is on, the simulator reports it in `subject` messages; while gestures are on, it
performs open palm, frame and spread in turn every few seconds and acts on them as the phone
does. Both are off until the desktop turns them on; `--tracking` and `--gestures` start with them
on, and `--gesture-seconds <s>` sets the pace.

For the away screen, `--room` swaps the moving pattern for a still picture and gives the person
a body, so the picture is empty and still while they are out (about five seconds in every
thirty). The Windows face detector does not take the orange square for a face, so this
exercises the movement side of the detection; faces are checked on photos, below.

To measure what the picture transform (crop, scale, rotate, mirror) costs per frame:

```bash
cargo test --release -p paircam-engine transform_cost -- --ignored --nocapture
```

To measure the colour stage (a look applied to a 1080p frame) and its table rebuild:

```bash
cargo test --release -p paircam-engine colour_cost -- --ignored --nocapture
```

To check the receiving pipeline from a terminal, without the interface:

```bash
cargo run -p paircam-engine --example headless -- 127.0.0.1:47810 --seconds 6 --save frame.bmp
```

The same example tries the away screen: `--away <seconds>` turns the automatic switch on with
that delay, `--away-media <file>` and `--away-message <text>` set what it shows, and
`--away-now` shows it from the start. It prints every change (someone in the picture or not,
the away screen showing or not) with its time, and `--save` then keeps the picture from one
second after the away screen first showed:

```bash
cargo run -p paircam-engine --example simulator -- --port 47816 --room
```

```bash
cargo run -p paircam-engine --example headless -- 127.0.0.1:47816 --seconds 26 --away 2 --away-media clip.mp4 --save away.bmp
```

## Checking the away screen

Two checks are for looking at, not for asserting, so they are ignored by default. The first
writes pictures of the away screen (the first picture, after one and three seconds of playing,
and after stopping) for a video or still picture, or for the plain card when
`PAIRCAM_AWAY_MEDIA` is not set; `PAIRCAM_AWAY_MESSAGE` changes the message:

```bash
PAIRCAM_AWAY_OUT=<folder> PAIRCAM_AWAY_MEDIA=<file> cargo test -p paircam-engine away_screen_pictures -- --ignored --nocapture
```

The second runs the Windows face detector over a folder of pictures, each cropped to a camera
frame and shrunk as the engine does, and prints where it found a face and how long it took:

```bash
PAIRCAM_FACE_PHOTOS=<folder> cargo test -p paircam-engine faces_in_photos -- --ignored --nocapture
```

Use only pictures you are licensed to use and keep them out of the repository. To see what the
detection reads from a live stream (face, movement, how much the seat changed), start the app
from a terminal with `PAIRCAM_TRACE=paircam_engine::presence`; the variable takes a comma
separated list of modules to log in full.

## Judging the looks

The looks are defined in `crates/engine/src/looks.rs`. Judge changes on real pictures, not on
the simulator's test pattern: the `looks` example applies every look to still images through
the same C colour stage and writes one contact sheet per image, plus a sheet of skin-tone chips,
grey ramps and each look's swatch strip:

```bash
cargo run --release -p paircam-engine --example looks -- --out <folder> --synthetic [image ...]
```

`--synthetic` adds three procedural portraits (fair, medium and deep skin under different
light). Use only pictures you are licensed to use (CC0 or your own) and keep them out of the
repository. `--time` times every look on each picture at 1920x1080 instead.

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

## Building the installer

```bash
powershell -ExecutionPolicy Bypass -File installer/build.ps1
```

This produces `target/installer/PairCam-Setup-<version>-x64.exe` from a clean checkout, without
administrator rights. The version is the workspace version in `Cargo.toml`. The script:

1. fetches FFmpeg if `third_party/ffmpeg` is missing;
2. runs `cargo build --release --locked -p paircam-desktop` and `native/vcam/build.ps1`;
3. fetches [Inno Setup](https://jrsoftware.org/isinfo.php) 7.1.1 into `third_party/` if it is
   missing. The download is pinned and checked against the SHA-256 GitHub publishes for the
   release and against its Authenticode signature, then unpacked in Inno Setup's portable,
   per-user mode (no UAC prompt, nothing installed for the whole machine);
4. stages what ships in `target/installer/stage`: `paircam.exe`, the FFmpeg DLLs listed in
   `crates/engine/build.rs`, `paircam_vcam.dll`, and the licences (PairCam's MIT licence,
   FFmpeg's LGPL licence and source notice, the Geist OFL, GPUI and GPUI Kit's Apache-2.0
   licences, and `THIRD-PARTY-NOTICES.txt` for every Rust crate in the exe). It checks with
   `dumpbin` that every DLL the binaries import is either staged or part of Windows;
5. compiles `installer/paircam.iss`. Compiler warnings fail the build.

`-SkipBuild` reuses the existing release build when only the installer script changed.

What the installer does, with one UAC prompt:

- installs into `C:\Program Files\PairCam` (fixed: the Windows camera service cannot read a
  folder under a user profile), with a Start menu shortcut, an optional desktop shortcut and an
  "Installed apps" entry;
- copies `paircam_vcam.dll` there, grants Local Service and Local System read access, and
  registers it machine-wide (`regsvr32`), like `scripts/register-vcam.ps1`. When upgrading while
  an app is using the camera, the loaded DLL is renamed aside and deleted at the next restart or
  install, so no restart is needed;
- closes a running PairCam before replacing it (Windows Restart Manager) and offers to launch
  it, as the user rather than elevated, at the end;
- follows the Windows display language (English or Turkish) and the light or dark theme.

The uninstaller closes PairCam, unregisters the camera, and removes everything except the
user's settings in `%APPDATA%\PairCam`. A camera DLL still in use is deleted at the next restart.
Silent installs work with `/VERYSILENT` (add `/TASKS=desktopicon` for the desktop shortcut);
Setup writes a log to `%TEMP%\Setup Log *.txt`.

The icon (`installer/assets/paircam.ico`, also embedded in `paircam.exe` by
`apps/desktop/build.rs` with the version resource) and the wizard images are drawn by
`installer/assets/make-icon.ps1` from the design tokens. Run it again after changing the mark
or the tokens, and commit its output.

### Code signing

The installer and `paircam.exe` are not signed yet. Until they are:

- Microsoft Defender SmartScreen shows "Windows protected your PC" for a downloaded setup file.
  Users continue with "More info", then "Run anyway". The UAC prompt shows "Publisher: Unknown".
- On PCs with Smart App Control turned on, Windows blocks unsigned apps without reputation and
  offers no override, so PairCam cannot be installed there.

Signing later needs a code signing certificate whose key is kept in hardware or a cloud HSM (an
OV certificate on a token, Azure Trusted Signing, or SignPath's free programme for open source
projects). Sign `paircam.exe` and `paircam_vcam.dll` before staging, then let Inno Setup sign
the setup and the uninstaller with a `SignTool` entry (`SignTool=` and `SignedUninstaller=yes` in
`paircam.iss`, and `/S<name>=...` on the compiler command line). SmartScreen still warns until
the certificate has built up reputation through downloads; an EV certificate no longer skips
that.
