# Roadmap

Each phase ends in something that can be used or shown. The first target is the iPhone app with
the Windows 11 desktop app; Android comes after that is solid.

## Phase 0: foundation

- [x] Repository, licence, working rules, architecture, design direction and protocol drafts
- [ ] Toolchain on the development PC: Windows 11 SDK, Expo and EAS accounts linked to the Apple
      Developer account (Rust is installed)
- [ ] Workspace tooling: Cargo workspace, TypeScript, lint, format, CI
- [ ] Design tokens and the generated desktop (GPUI Kit) and phone themes
- [ ] Desktop component gallery in both themes, approved before any screen is built
- [ ] Translation catalogs (English source, device language, Turkish as the first translation) in both apps
- [ ] Virtual camera spike: a test pattern visible as "PairCam" in Zoom, Teams, Meet (Chrome),
      Discord and OBS
- [ ] Preview spike: live video drawn smoothly in a GPUI window on Windows
- [ ] Phone build pipeline: EAS Build to TestFlight, with an empty app installed on a real iPhone
- [ ] Remote diagnostics: phone logs and stats visible in the desktop app

## Phase 1: first video

- [ ] iOS camera engine: 1080p at 30 fps, HEVC
- [ ] Desktop engine: receive, GPU decode, feed the virtual camera
- [ ] QR pairing and automatic reconnect
- [ ] Live preview and connection state in the desktop app
- [ ] Placeholder image in the camera when no phone is connected

## Phase 2: quality

- [ ] 4K and 60 fps, bitrate and quality controls
- [ ] All lenses, zoom, focus, exposure and white-balance control from the desktop
- [ ] Stabilization, HDR, low-light handling
- [ ] Heat and battery protection: gradual quality reduction, dimmed screen
- [ ] Delay and smoothness measured and tuned against DroidCam, Camo and Iriun

## Phase 3: product

- [ ] Custom title bar, tray, start with Windows
- [ ] Installer that registers the camera, signed builds, silent auto-update
- [ ] USB connection
- [ ] App Store release

## Phase 4: reach

- [ ] Android app
- [ ] Phone microphone as a PC microphone
- [ ] Multiple phones at once
- [ ] macOS desktop app
