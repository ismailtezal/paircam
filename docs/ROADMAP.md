# Roadmap

Each phase ends in something that can be used or shown.

## Phase 0: foundation

- [x] Repository, licence, working rules, architecture and protocol drafts
- [ ] Toolchain on the development PC: Rust, Windows 11 SDK, Expo and EAS accounts linked
- [ ] Monorepo tooling: workspaces, TypeScript, lint, format, CI
- [ ] Design tokens and the generated desktop and phone themes
- [ ] Desktop component set and a design-system page, approved before any screen is built
- [ ] Translation catalogs (English source, device language, Turkish as the first translation) in both apps
- [ ] Virtual camera spike: a test pattern visible as "Kadraj Camera" in Zoom, Teams, Meet (Chrome),
      Discord and OBS
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
