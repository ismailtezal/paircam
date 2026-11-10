# Kadraj protocol

**Version 0: draft.** This will change until the first working end-to-end build; after that,
every change bumps the version and updates the test vectors in `packages/protocol`.

## Goals

- Lowest practical delay on a local network.
- The same bytes over Wi-Fi and over a USB tunnel.
- Both directions: video from the phone, control from the desktop.
- Old and new app versions can detect each other and say so clearly.

## Discovery and pairing

- The desktop app advertises `_kadraj._tcp` over mDNS. The phone lists what it finds.
- First pairing: the desktop shows a QR code holding its address, port and a one-time secret.
  The phone scans it, both sides derive a long-term key, and later connections are automatic.
- Over Wi-Fi the phone connects to the desktop (the desktop listens). Over USB the direction is
  reversed, because the tunnel only lets the PC open connections; the messages are identical.
- The connection is encrypted and authenticated with the pairing key. Unpaired devices get
  nothing.

## Transport

One TCP connection with `TCP_NODELAY`, carrying framed messages. On a local network, TCP with
frame dropping at the sender keeps delay low and is far simpler than a custom UDP transport.
If measurements on poor Wi-Fi show head-of-line stalls, version 1 may move video to UDP.

## Framing

All integers are big-endian.

```
u8   type
u32  length      payload bytes, excluding this 5-byte header
...  payload
```

## Messages

| Type | Name | Direction | Payload |
| --- | --- | --- | --- |
| 0x01 | hello | both | `u8` protocol version, then UTF-8 JSON: app version, device name, platform |
| 0x02 | capabilities | phone → desktop | JSON: cameras, lenses, formats (size, frame rates, codecs), supported controls and their ranges |
| 0x03 | configure | desktop → phone | JSON: camera, size, frame rate, codec, bitrate, keyframe interval |
| 0x04 | videoConfig | phone → desktop | `u8` codec (0 = H.264, 1 = HEVC), `u16` width, `u16` height, `u16` rotation, `u32` length, decoder configuration record (`avcC` / `hvcC`) |
| 0x05 | videoFrame | phone → desktop | `u64` capture timestamp in microseconds, `u8` flags (bit 0 = keyframe), one access unit |
| 0x06 | control | desktop → phone | JSON: one control change (zoom, focus, exposure, white balance, torch, mirror…) |
| 0x07 | controlState | phone → desktop | JSON: current value of every control |
| 0x08 | requestKeyframe | desktop → phone | empty |
| 0x09 | stats | phone → desktop | JSON: frame rate, bitrate, dropped frames, temperature state, battery |
| 0x0A | log | phone → desktop | JSON: level, tag, message (remote diagnostics) |
| 0x0B | ping | both | `u64` sender timestamp |
| 0x0C | pong | both | the `u64` timestamp from the ping, for delay measurement |
| 0x0F | bye | both | JSON: reason |

Unknown message types are skipped using the length field, so new messages do not break old apps.

## Flow control

The sender tracks bytes queued but not yet written. Above a threshold it stops queueing video,
drops frames until the next keyframe, and reports the drop in `stats`. If the peer reads nothing
for five seconds, the connection is closed and re-established.

## Audio

Not in version 0. Message types 0x10 to 0x1F are reserved for it.
