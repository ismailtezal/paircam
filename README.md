# Kadraj

Use your phone as a high-quality webcam on your PC, over Wi-Fi or USB. Free and open source.

> **Status: pre-alpha.** Nothing is usable yet. This repository currently holds the plan and the
> foundation; see [docs/ROADMAP.md](docs/ROADMAP.md).

## Why

Existing phone-as-webcam apps either lock HD, extra lenses and watermark-free video behind a
subscription, or they are unreliable. Kadraj aims to be the one you would pick even if the others
were free: full camera quality, low delay, and a desktop app that feels like a real product.

## How it works

```
Phone app ──(Wi-Fi or USB)──▶ Desktop app ──▶ "Kadraj Camera" in Zoom, Teams, Meet, Discord, OBS
```

- The **phone** captures the camera and compresses it with its hardware encoder (HEVC or H.264).
- The **desktop app** receives the stream, decodes it on the graphics card, and feeds a virtual
  camera that Windows 11 shows to every app like a real webcam.
- The desktop app also controls the phone: lens, zoom, focus, exposure, resolution, frame rate.

| Part | Built with |
| --- | --- |
| Phone app (iOS, Android) | React Native (Expo) for the interface; native camera engines in Swift and Kotlin |
| Desktop app (Windows 11) | Tauri 2, React, Tailwind CSS, shadcn/ui |
| Desktop video engine | Rust, FFmpeg with hardware decoding |
| Virtual camera | C++, Windows 11 Media Foundation virtual camera (no driver) |

More detail: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/PROTOCOL.md](docs/PROTOCOL.md),
[docs/PRIOR-ART.md](docs/PRIOR-ART.md).

## License

[MIT](LICENSE)
