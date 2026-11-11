# Architecture

## Pipeline

```
┌──────────── iPhone ───────────┐            ┌────────────────── Windows PC ──────────────────┐
│ camera → hardware encoder     │  Wi-Fi /   │ receiver → GPU decoder → frame channel → vcam  │
│ (HEVC / H.264)   → sender ────┼── USB ────▶│                                          │     │
│ React Native UI ◀─ control ◀──┼────────────┼── control ◀── desktop UI          Zoom, Teams… │
└───────────────────────────────┘            └────────────────────────────────────────────────┘
```

1. The phone captures frames and compresses them with its hardware encoder. The PC never
   re-encodes; it only decodes.
2. Nothing is buffered. The desktop shows each frame as soon as it is decoded. If the link falls
   behind, the sender drops frames and resumes at the next keyframe, so delay never accumulates.
3. The desktop engine hands decoded frames to the virtual camera, which keeps only the newest
   frame and shows a placeholder image when no phone is connected.
4. A control channel runs the other way: the desktop app changes lens, zoom, focus, exposure,
   resolution, frame rate and bitrate on the phone, and the phone reports stats and logs back.

## Repository layout

Folders are created when their first code lands. The Rust code is one Cargo workspace.

| Path | Contents |
| --- | --- |
| `apps/desktop` | The desktop app: a Rust binary built on GPUI. Windows, screens, tray. |
| `apps/mobile` | The iPhone app: React Native (Expo, development builds). Interface, pairing, settings. |
| `apps/mobile/modules/camera-engine` | Expo native module in Swift: capture (AVFoundation), encode (VideoToolbox), send. |
| `crates/ui` | The PairCam theme and shared components on top of GPUI Kit. |
| `crates/engine` | Discovery, pairing, receiver, FFmpeg hardware decoding, frame channel. |
| `crates/protocol` | The protocol in Rust: message types, framing, encode and decode. |
| `native/vcam` | C++ Media Foundation virtual camera source (a COM DLL loaded by Windows Frame Server). |
| `design/tokens` | Design tokens; generates the GPUI Kit theme and the phone theme. |
| `protocol/vectors` | Binary test vectors shared by the Rust and Swift protocol tests. |
| `docs` | Specifications and plans. |

## iPhone app

React Native owns everything the user sees and none of the video path. The camera engine is a
native Swift module with a small API: list cameras and formats, start and stop, apply a control,
and emit stats. Frames stay in native memory from the sensor to the socket.

- **Capture and encode:** AVFoundation and VideoToolbox.
- **Foreground only:** iOS stops the camera when the app leaves the foreground, so the app keeps
  the screen awake and dims it while streaming.
- **Builds:** EAS Build produces the app in the cloud, so no Mac is required. Builds are
  distributed through TestFlight with the Apple Developer account, then the App Store.
- **Android later:** the React Native interface is kept free of iOS-only assumptions, and the
  camera engine API is the seam where a Kotlin engine will plug in.

## Desktop app

The whole desktop app is Rust in one process: interface, engine and the feed to the camera.

- **Interface:** GPUI, the GPU-accelerated UI framework from Zed, with GPUI Kit components under
  the PairCam theme. No web view. Its own title bar, tray icon, start with Windows, auto-update.
- **Engine:** finds phones on the network, pairs, receives the stream, decodes with FFmpeg on the
  GPU (D3D11VA), and converts to NV12. Bundling FFmpeg means HEVC works without the paid
  Microsoft HEVC extension.
- **Discovery:** the phone advertises itself with Bonjour and the engine finds it through the
  DNS-SD client built into Windows. The desktop always opens the connection, so Windows never
  shows a firewall prompt.
- **Picture transform:** one place turns each decoded picture into what other apps see. The
  decoding thread only copies the picture; a picture thread asks the framer for this frame's
  crop (the user's zoom and pan, or the face tracking framing, eased with a critically damped
  spring) and the C layer crops, scales, turns, mirrors and flips it in one pass into the
  1920x1080 camera frame, fitted, filled or stretched. The settings are kept per computer.
- **Preview:** the same plan, scaled to the preview size as BGRA (up to 60 a second), so the
  preview shows exactly what other apps get. While an app uses the camera, the preview is
  scaled down from the camera frame itself; otherwise the transform renders straight at the
  preview size. The virtual camera gets every picture regardless of the preview.
- **Virtual camera (C++):** a Media Foundation source registered once, with administrator
  rights, by the installer. Windows loads it into its own camera service, so it must live in a
  folder that service can read (Program Files). The engine feeds it over a named pipe or shared
  memory. Requires Windows 11 (build 22000 or later).
- **USB:** Apple's Mobile Device Service tunnels a normal TCP connection over the cable, so USB
  reuses the Wi-Fi code path.

## Platform targets

| Platform | Status |
| --- | --- |
| Windows 11 desktop | First target |
| iPhone | First target |
| Android | Later; same React Native interface, Kotlin engine |
| macOS, Linux desktop | Later; GPUI runs there, but each needs its own virtual camera |
| Windows 10 | Not planned; it lacks the driverless virtual camera |
