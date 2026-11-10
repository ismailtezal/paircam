# Architecture

## Pipeline

```
┌──────────── phone ────────────┐            ┌────────────────── Windows PC ──────────────────┐
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

Folders are created when their first code lands.

| Path | Contents |
| --- | --- |
| `apps/mobile` | React Native app (Expo, development builds). Interface, pairing, settings. |
| `apps/mobile/modules/camera-engine` | Expo native module: capture, encode and send. Swift (AVFoundation, VideoToolbox) and Kotlin (Camera2, MediaCodec). |
| `apps/desktop` | Tauri 2 app: React, Tailwind CSS, shadcn/ui. The Rust side hosts the engine. |
| `native/engine` | Rust crate: discovery, pairing, receiver, FFmpeg hardware decoding, frame channel. |
| `native/vcam` | C++ Media Foundation virtual camera source (a COM DLL loaded by Windows Frame Server). |
| `packages/tokens` | Design tokens; generates CSS variables and the phone theme. |
| `packages/protocol` | Protocol constants, TypeScript types and shared test vectors. |
| `docs` | Specifications and plans. |

## Phone app

React Native owns everything the user sees and none of the video path. The camera engine is a
native module with a small API: list cameras and formats, start and stop, apply a control, and
emit stats. Frames stay in native memory from the sensor to the socket.

- **iOS:** AVFoundation capture, VideoToolbox encoding. iOS stops the camera when the app leaves
  the foreground, so the app keeps the screen awake and dims it while streaming.
- **Android:** Camera2 capture, MediaCodec encoding, a foreground service so streaming survives
  the screen turning off.
- **Builds:** EAS Build produces both apps in the cloud, so no Mac is required. iOS builds are
  distributed through TestFlight with the Apple Developer account.

## Desktop app

- **Interface:** Tauri 2 with React. Its own title bar, tray icon, start with Windows, auto-update.
- **Engine (Rust):** finds phones on the network, pairs, receives the stream, decodes with FFmpeg
  on the GPU (D3D11VA), and converts to NV12. Bundling FFmpeg means HEVC works without the paid
  Microsoft HEVC extension.
- **Virtual camera (C++):** a Media Foundation source registered once, with administrator
  rights, by the installer. Windows loads it into its own camera service, so it must live in a
  folder that service can read (Program Files). The engine feeds it over a named pipe or shared
  memory. Requires Windows 11 (build 22000 or later).
- **USB:** on iOS, Apple's Mobile Device Service tunnels a normal TCP connection over the cable;
  on Android, `adb forward` does the same. Both reuse the Wi-Fi code path.

## Platform targets

| Platform | Status |
| --- | --- |
| Windows 11 desktop | First target |
| iOS | First target |
| Android | Second target; same React Native interface, Kotlin engine |
| macOS, Linux desktop | Later; needs a different virtual camera per system |
| Windows 10 | Not planned; it lacks the driverless virtual camera |
