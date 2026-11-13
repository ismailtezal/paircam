# PairCam protocol

**Version 0.** This may still change until the first release; every change after that bumps the
version. The Rust implementation is `crates/protocol`; the Swift implementation lives in the
phone's camera engine. Both are tested against the shared vectors in `protocol/vectors`.

## Goals

- Lowest practical delay on a local network.
- The same bytes over Wi-Fi and over a USB tunnel.
- Both directions: video from the phone, control from the desktop.
- Nothing readable or controllable by other devices on the network.
- Old and new app versions can detect each other and say so clearly.

## Roles and transport

- The **camera** is the phone. It listens on a TCP port (default `47810`).
- The **host** is the desktop app. It always opens the connection.

The host connects outward for two reasons: Windows does not ask the user to approve outgoing
connections, and a USB tunnel only lets the PC open connections. Wi-Fi and USB therefore carry
identical bytes.

One TCP connection with `TCP_NODELAY` carries everything. On a local network, TCP with frame
dropping at the sender keeps delay low and is far simpler than a custom UDP transport. If
measurements on poor Wi-Fi show head-of-line stalls, a later version may move video to UDP.

## Discovery

The camera advertises the Bonjour service `_paircam._tcp` while the app is open, with TXT records:

| Key | Value |
| --- | --- |
| `v` | protocol version, decimal |
| `id` | camera id, 32 hex characters |
| `name` | device name shown to the user |

The host browses for the service and lists the cameras it finds. A camera can also be reached by
typing its address.

## Records

All integers are big-endian. Everything on the wire is a sequence of records:

```
u32  length      bytes that follow, at most 8 MiB
...  body
```

The first three records are the plaintext handshake. Every record after that is encrypted.

## Handshake

Each side has a random 16-byte device id that never changes. Each connection uses fresh X25519
key pairs.

| # | Direction | Body |
| --- | --- | --- |
| 1 | host → camera | `"PCAM"`, `u8` version, `u8` mode (0 = pair, 1 = resume), 16-byte host id, 32-byte commitment = SHA-256(host public key) |
| 2 | camera → host | `"PCAM"`, `u8` version, `u8` status, 16-byte camera id, 32-byte camera public key |
| 3 | host → camera | 32-byte host public key |

Status: 0 = ok, 1 = unsupported version, 2 = not paired with this host (in resume mode),
3 = busy with another host. Any status other than 0 ends the connection.

The camera checks that record 3 matches the commitment in record 1 and closes if it does not.
Committing first stops an attacker in the middle from choosing a key that makes the pairing codes
match.

### Keys

```
dh         = X25519(own private key, peer public key)
psk        = 32 zero bytes                      (pair mode)
           = the stored pairing key             (resume mode)
prk        = HKDF-Extract(salt = psk, ikm = dh)                 HKDF-SHA-256
th         = SHA-256(record 1 body || record 2 body || record 3 body)
key(label) = HKDF-Expand(prk, info = label || th, 32 bytes)

host → camera key   label "paircam v0 h2c"
camera → host key   label "paircam v0 c2h"
pairing key         label "paircam v0 pairing"
pairing code        label "paircam v0 sas", first 4 bytes as u32, modulo 1 000 000,
                    shown as 6 digits with leading zeros
```

### Encrypted records

Each record body after the handshake is `ChaCha20-Poly1305(key, nonce, message)` with no
associated data. The nonce is 4 zero bytes followed by a `u64` counter that starts at 0 for each
direction and increases by one per record. A record that fails to decrypt ends the connection.

A decrypted message is:

```
u8   type
...  payload
```

### Pairing and resuming

After record 3, both sides send `hello`. Being able to decrypt the peer's `hello` proves the peer
derived the same keys.

- **Pair mode.** Both apps show the pairing code. The user confirms on the phone that the codes
  match. The camera then sends `pairResult`. If accepted, both sides store the pairing key under
  the peer's device id. Until then the camera ignores every other message.
- **Resume mode.** The pairing key is mixed into the keys, so only a paired host can produce a
  `hello` the camera can decrypt. No user action is needed.

## Messages

JSON payloads are UTF-8 objects. Unknown fields are ignored and unknown message types are skipped,
so newer apps can add both without breaking older ones.

| Type | Name | Direction | Payload |
| --- | --- | --- | --- |
| 0x01 | hello | both | JSON: `name`, `appVersion`, `platform`, `model` |
| 0x02 | pairResult | camera → host | JSON: `accepted` |
| 0x03 | capabilities | camera → host | JSON, see below |
| 0x04 | configure | host → camera | JSON, see below |
| 0x05 | startStream | host → camera | empty |
| 0x06 | stopStream | host → camera | empty |
| 0x10 | videoConfig | camera → host | binary, see below |
| 0x11 | videoFrame | camera → host | binary, see below |
| 0x12 | requestKeyframe | host → camera | empty |
| 0x20 | control | host → camera | JSON: one control change |
| 0x21 | controlState | camera → host | JSON: current value of every control |
| 0x22 | subject | camera → host | JSON: where the person is in the picture, while face tracking is on |
| 0x23 | gesture | camera → host | JSON: a hand gesture the camera recognised |
| 0x24 | cinematic | camera → host | JSON: what Cinematic mode sees and where it focuses |
| 0x30 | stats | camera → host | JSON, about once a second while streaming |
| 0x31 | log | camera → host | JSON: `level`, `tag`, `message` |
| 0x3E | ping | both | `u64` sender timestamp in microseconds |
| 0x3F | pong | both | the `u64` from the ping |
| 0x7F | bye | both | JSON: `reason` |

Types 0x18 to 0x1F are reserved for audio.

### capabilities

Sent by the camera after `hello` (after `pairResult` in pair mode), and again whenever they change.

```json
{
  "cameras": [
    {
      "id": "back-wide",
      "name": "Back Camera",
      "position": "back",
      "lens": "wide",
      "formats": [{ "width": 1920, "height": 1080, "maxFps": 60 }]
    }
  ],
  "codecs": ["hevc", "h264"],
  "controls": {
    "zoom": { "min": 1.0, "max": 10.0 },
    "exposureBias": { "min": -8.0, "max": 8.0 },
    "focus": ["auto", "locked", "manual"],
    "whiteBalance": ["auto", "locked"],
    "torch": true,
    "stabilization": ["off", "standard", "cinematic"]
  }
}
```

`position` is `front`, `back` or `external`. `lens` is `wide`, `ultraWide`, `telephoto` or
`other`. A control the camera does not support is absent.

### configure

Chooses what to stream. If the stream is running it restarts with the new settings.

```json
{
  "camera": "back-wide",
  "width": 1920,
  "height": 1080,
  "fps": 30,
  "codec": "hevc",
  "bitrate": 12000000,
  "keyframeInterval": 2.0
}
```

`bitrate` is in bits per second and `keyframeInterval` in seconds.

### videoConfig

Sent before the first frame and whenever the stream parameters change.

```
u8   codec       0 = H.264, 1 = HEVC
u16  width       encoded picture width in pixels
u16  height      encoded picture height in pixels
u16  rotation    clockwise degrees the host must rotate the picture: 0, 90, 180 or 270
u8   flags       bit 0 = mirror horizontally (front camera)
u32  length
...  decoder configuration record: `avcC` for H.264, `hvcC` for HEVC
```

In version 0 the camera rotates and mirrors the picture itself, so the encoded frames are
already upright: it always sends `rotation` 0 and the mirror flag clear, with the width and
height as encoded, and the host ignores both fields. When the phone turns, the camera restarts
its encoder and sends a new `videoConfig` followed by a keyframe.

### videoFrame

```
u64  capture timestamp in microseconds, on the camera's monotonic clock
u8   flags       bit 0 = keyframe
...  one access unit: NAL units, each preceded by its u32 length
```

This is the layout the phone's hardware encoder produces, so frames are sent without rewriting.

### control and controlState

`control` changes one thing; `controlState` reports everything.

```json
{ "zoom": 2.0 }
{ "focus": "manual", "focusPosition": 0.4 }
{ "focusPoint": { "x": 0.5, "y": 0.5 } }
{ "exposureBias": -0.5 }
{ "whiteBalance": "locked" }
{ "torch": true }
{ "stabilization": "standard" }
```

### stats

```json
{
  "fps": 29.97,
  "bitrate": 11840000,
  "droppedFrames": 0,
  "thermal": "nominal",
  "battery": 0.82,
  "charging": true
}
```

`thermal` is `nominal`, `fair`, `serious` or `critical`.

### Face tracking and hand gestures

Both are off unless the host or the user turns them on. They are announced in `capabilities`
and switched in `control`, like every other control:

```json
"controls": { "faceTracking": true, "gestures": ["palm", "frame", "cross", "spread"] }
```

```json
{ "faceTracking": true }
{ "gestures": true }
```

`controlState` reports both as booleans. A camera that supports neither omits them.

**Face tracking.** The camera only finds the person; the host does the framing, because the
host is where the picture is cropped for other apps. While `faceTracking` is on, the camera
sends `subject` up to 15 times a second:

```json
{ "face": { "x": 0.41, "y": 0.22, "width": 0.18, "height": 0.24 } }
{ "face": null }
```

The rectangle is the face in the encoded picture, with the origin at the top left and both axes
from 0 to 1. `null` means nobody is in view. The host moves its crop smoothly; the camera sends
raw detections and does no smoothing of its own.

**Gestures.** While `gestures` is on, the camera watches for hand gestures in its own video and
acts on them itself, then tells the host so the interface can show what happened:

| Gesture | Meaning | What the camera does |
| --- | --- | --- |
| `palm` | An open palm held up for about a second | In Cinematic mode, pulls the focus to the hand, or back to the camera's own choice if the focus is already held there. Otherwise toggles `faceTracking` and sends `controlState` |
| `frame` | Thumb and index finger in an L shape, held for about a second | Zooms in one step (2×) |
| `cross` | Two index fingers crossed in an X, held for about a second | Zooms out one step, never below 1× |
| `spread` | Two hands moving apart or together | Zooms in or out continuously while the hands move |

```json
{ "kind": "palm", "phase": "recognised" }
{ "kind": "spread", "phase": "began" }
{ "kind": "spread", "phase": "ended" }
```

`phase` is `recognised` for a one-off gesture, and `began` then `ended` for a continuous one.
Zoom changes always ramp smoothly on the camera, whether they come from a gesture or from the
host, and the camera reports the new value in `controlState` when the ramp settles.

### Cinematic mode

The camera films like a film camera with a wide aperture: the background melts into lens
blur, and the focus racks from one subject to another as they come, go and turn. The camera
renders the effect itself, so the frames the host receives already have it. A camera that can
do it announces it in `capabilities`:

```json
"controls": { "cinematic": { "positions": ["back", "front"], "aperture": { "min": 2.0, "max": 16.0 } } }
```

`positions` are the sides of the phone with a camera that can film this way. With Cinematic mode
on, the camera films from that camera on the side of the camera chosen in `configure` (on an
iPhone, the back Dual Wide camera or the front TrueDepth camera), at 30 frames a second or less
and at a size it supports, and reports the real size in `videoConfig`. `aperture` is the range
of the simulated aperture as f-numbers: the smaller the number, the stronger the blur.

`control` turns it on and off, sets the aperture, and moves the focus. `controlState` reports
`cinematic` and `aperture`:

```json
{ "cinematic": true }
{ "aperture": 2.8 }
{ "cinematicFocus": { "mode": "auto" } }
{ "cinematicFocus": { "mode": "subject", "id": 12, "strong": true } }
{ "cinematicFocus": { "mode": "track", "point": { "x": 0.25, "y": 0.75 }, "strong": false } }
{ "cinematicFocus": { "mode": "fixed", "point": { "x": 0.5, "y": 0.5 } } }
```

`cinematicFocus` acts once, like a tap or a long press on the camera's own screen:

| Mode | What the camera does |
| --- | --- |
| `auto` | Decides again by itself |
| `subject` | Focuses on the subject with that `id` from the last `cinematic` message |
| `track` | Finds whatever is at the point and follows it |
| `fixed` | Holds the focus at the distance of whatever is at the point, whatever moves |

Points use the encoded picture, origin at the top left, both axes from 0 to 1. A `strong` focus
stays on its subject until it leaves the picture; a weak one (`strong` false or absent) lets the
camera rack to another subject that becomes more prominent. A `fixed` focus is always strong.
A camera ignores a mode it does not know.

While Cinematic mode is on, the camera sends `cinematic` whenever what it sees changes, up to
10 times a second:

```json
{
  "subjects": [
    { "id": 12, "kind": "face", "rect": { "x": 0.375, "y": 0.25, "width": 0.1875, "height": 0.25 }, "focus": "weak" },
    { "id": 13, "kind": "object", "rect": { "x": 0.625, "y": 0.5, "width": 0.125, "height": 0.125 }, "focus": "none" }
  ],
  "lowLight": false
}
```

`kind` is `face`, `head`, `body`, `hand`, `animal`, `object` or `fixed` (a fixed focus distance
rather than a subject). `focus` is `none`, `weak` or `strong`; at most one subject is in
focus. `rect` uses the encoded picture like `subject`. `lowLight` says the room is too dark for
the effect to work well. A subject keeps its `id` while the camera keeps seeing it.

## Flow control

The sender tracks bytes queued but not yet written. Above a threshold it stops queueing video,
drops frames until the next keyframe, and counts the drops in `stats`. If the peer reads nothing
for five seconds, the connection is closed. Either side may send `ping` at any time; a peer that
does not answer within five seconds is treated as gone. The host pings every second and the
camera every two, so both must answer `ping` with `pong`.

## Ending a session

`bye` carries a `reason`. The camera uses `stopped` (the user tapped Stop), `declined`,
`pairingTimedOut`, `backgrounded` (the app left the foreground, which stops the camera on iOS)
and `forgotten` (the user removed this computer). The host uses `closed by the user`. After
`stopped` the host does not reconnect by itself. Unknown reasons are treated as a plain close.

The camera serves one host at a time and answers others with status 3 (busy). A host that
reconnects right after a dropped connection can get busy for up to five seconds, until the
camera gives up on the old connection.
