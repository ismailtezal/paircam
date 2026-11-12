# Roadmap

Each phase ends in something that can be used or shown. The first target is the iPhone app with
the Windows 11 desktop app; Android comes after that is solid.

A box is ticked only when the thing has been seen working. "Written" means the code exists and
passes the checks that can run on Windows, but has not run on a real device yet.

## Phase 0: foundation

- [x] Repository, licence, working rules, architecture, design direction and protocol
- [x] Toolchain on the development PC: Rust, Windows 11 SDK, pinned FFmpeg
- [x] Workspace tooling: Cargo workspace, lint, format, CI workflow
- [x] Design tokens and the generated desktop (GPUI Kit) and phone themes
- [x] Desktop component gallery in both themes (`paircam --gallery`)
- [x] Translation catalogs (English source, device language, Turkish) in both apps
- [x] Preview spike: live video drawn smoothly in a GPUI window on Windows
- [x] A phone simulator, so the desktop can be built and tested without an iPhone
- [ ] Virtual camera seen in Zoom, Teams, Meet (Chrome), Discord and OBS. Written and tested
      with a harness; needs the one-time install with administrator rights, then real apps.
- [ ] Phone build pipeline: EAS Build to TestFlight on a real iPhone. Configured; needs the
      Expo and Apple accounts linked.
- [ ] Remote diagnostics: phone logs and stats visible in the desktop app. Phone stats are shown
      and phone log lines are saved to `phone.log` in the app data folder; there is no log viewer
      in the interface yet.

## Phase 1: first video

- [x] Desktop engine: discover, pair, receive, GPU decode, feed the virtual camera
- [x] Pairing with a 6-digit code and automatic reconnect to paired phones
- [x] Live preview, stats and connection state in the desktop app
- [ ] Placeholder picture in the camera when no phone is connected. Written; drawn correctly in
      the test harness, not yet seen in a real app.
- [ ] iOS camera engine: 1080p at 30 fps, HEVC. Written; not yet compiled or run.
- [ ] The first stream from a real iPhone to a real app

## Phase 2: quality

- [ ] 4K and 60 fps end to end (the desktop and protocol already carry them; the virtual camera
      is 1080p at 30 fps for now)
- [x] Camera, resolution, frame rate, quality and format chosen from the desktop
- [x] Zoom, exposure, flashlight, focus lock and white-balance lock from the desktop
- [ ] Stabilization, HDR, low-light handling
- [ ] Heat and battery protection: gradual quality reduction
- [ ] Delay and smoothness measured and tuned against DroidCam, Camo and Iriun

## Phase 3: product

- [x] Custom title bar
- [ ] Tray icon, start with Windows
- [ ] Installer that registers the camera, signed builds, silent auto-update
- [ ] Away screen: a video, picture or card with a message while nobody is in the picture,
      automatic or by hand. Written; seen working end to end with the simulator, not yet with
      a real phone and a real person, where the detection thresholds still need tuning.
- [ ] USB connection
- [ ] App Store release

## Phase 4: reach

- [ ] Android app
- [ ] Phone microphone as a PC microphone
- [ ] Multiple phones at once
- [ ] macOS desktop app
