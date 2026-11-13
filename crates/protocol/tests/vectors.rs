//! Keeps `protocol/vectors/v0.json` in step with this implementation.
//!
//! The Swift implementation on the phone is tested against the same file. Regenerate it after an
//! intended protocol change with:
//!
//! ```text
//! PAIRCAM_UPDATE_VECTORS=1 cargo test -p paircam-protocol --test vectors
//! ```

use std::path::PathBuf;

use bytes::Bytes;
use paircam_protocol::{
    CameraHandshake, Capabilities, CinematicCapabilities, CinematicFocus, CinematicState,
    CinematicSubject, Codec, ControlCapabilities, ControlValues, DeviceId, Gesture, GestureKind,
    GesturePhase, Hello, HostHandshake, Message, Mode, Opener, PairingKey, Point, Position, Range,
    Rect, Sealer, SessionKeys, Subject, SubjectFocus, SubjectKind, UNPAIRED, VideoConfig,
    VideoFrame,
};
use serde_json::{Value, json};

fn sequence<const N: usize>(start: u8) -> [u8; N] {
    std::array::from_fn(|index| start.wrapping_add(index as u8))
}

struct Handshake {
    vector: Value,
    keys: SessionKeys,
}

fn handshake(mode: Mode, pairing_key: &PairingKey, key_seed: u8) -> Handshake {
    let host_id = DeviceId(sequence(0x00));
    let camera_id = DeviceId(sequence(0x10));
    let host_private_key: [u8; 32] = sequence(key_seed);
    let camera_private_key: [u8; 32] = sequence(key_seed.wrapping_add(0x40));

    let host = HostHandshake::new(host_id, mode, host_private_key);
    let record1 = host.hello().to_vec();

    let host_hello = CameraHandshake::read_host_hello(&record1).unwrap();
    let camera = CameraHandshake::accept(&host_hello, camera_id, camera_private_key);
    let record2 = camera.hello().to_vec();

    let camera_hello = HostHandshake::read_camera_hello(&record2).unwrap();
    let (record3, keys) = host.finish(&camera_hello, pairing_key).unwrap();
    let camera_keys = camera.finish(&record3, pairing_key).unwrap();
    assert_eq!(keys, camera_keys);

    let vector = json!({
        "mode": match mode { Mode::Pair => "pair", Mode::Resume => "resume" },
        "hostId": host_id.to_hex(),
        "cameraId": camera_id.to_hex(),
        "hostPrivateKey": hex::encode(host_private_key),
        "cameraPrivateKey": hex::encode(camera_private_key),
        "pairingKeyIn": hex::encode(pairing_key),
        "record1": hex::encode(&record1),
        "record2": hex::encode(&record2),
        "record3": hex::encode(&record3),
        "hostToCameraKey": hex::encode(keys.host_to_camera),
        "cameraToHostKey": hex::encode(keys.camera_to_host),
        "pairingKeyOut": hex::encode(keys.pairing_key),
        "pairingCode": keys.pairing_code_digits(),
    });
    Handshake { vector, keys }
}

fn sealed_records(key: &[u8; 32]) -> Value {
    let mut sealer = Sealer::new(key);
    let mut opener = Opener::new(key);
    let plaintexts: [&[u8]; 3] = [b"", b"\x05", b"PairCam record number three"];

    let records: Vec<Value> = plaintexts
        .iter()
        .enumerate()
        .map(|(counter, plaintext)| {
            let mut record = plaintext.to_vec();
            sealer.seal(&mut record).unwrap();
            let sealed = hex::encode(&record);
            opener.open(&mut record).unwrap();
            assert_eq!(&record, plaintext);
            json!({
                "counter": counter,
                "plaintext": hex::encode(plaintext),
                "record": sealed,
            })
        })
        .collect();

    json!({ "key": hex::encode(key), "records": records })
}

fn messages() -> Value {
    let binary = [
        (
            "videoConfig",
            Message::VideoConfig(VideoConfig {
                codec: Codec::Hevc,
                width: 1920,
                height: 1080,
                rotation: 90,
                mirrored: true,
                record: Bytes::from_static(&[0x01, 0x02, 0x03, 0x04, 0x05]),
            }),
        ),
        (
            "videoFrame",
            Message::VideoFrame(VideoFrame {
                timestamp_us: 0x0001_0203_0405_0607,
                keyframe: true,
                data: Bytes::from_static(&[0x00, 0x00, 0x00, 0x02, 0x26, 0x01]),
            }),
        ),
        ("startStream", Message::StartStream),
        ("stopStream", Message::StopStream),
        ("requestKeyframe", Message::RequestKeyframe),
        ("ping", Message::Ping(1_000_000)),
        ("pong", Message::Pong(1_000_000)),
    ];
    let json_messages = [
        (
            "hello",
            Message::Hello(Hello {
                name: "Test iPhone".into(),
                app_version: "0.1.0".into(),
                platform: "ios".into(),
                model: "iPhone16,2".into(),
            }),
        ),
        (
            "control",
            Message::Control(ControlValues {
                zoom: Some(2.0),
                torch: Some(true),
                ..Default::default()
            }),
        ),
        (
            "controlTracking",
            Message::Control(ControlValues {
                face_tracking: Some(true),
                gestures: Some(false),
                ..Default::default()
            }),
        ),
        (
            "capabilitiesTracking",
            Message::Capabilities(Capabilities {
                controls: ControlCapabilities {
                    face_tracking: true,
                    gestures: vec![GestureKind::Palm, GestureKind::Frame, GestureKind::Spread],
                    ..Default::default()
                },
                ..Default::default()
            }),
        ),
        (
            "subject",
            Message::Subject(Subject {
                face: Some(Rect {
                    x: 0.375,
                    y: 0.25,
                    width: 0.1875,
                    height: 0.25,
                }),
            }),
        ),
        ("subjectNone", Message::Subject(Subject { face: None })),
        (
            "gesture",
            Message::Gesture(Gesture {
                kind: GestureKind::Palm,
                phase: GesturePhase::Recognised,
            }),
        ),
        (
            "gestureBegan",
            Message::Gesture(Gesture {
                kind: GestureKind::Spread,
                phase: GesturePhase::Began,
            }),
        ),
        (
            "gestureCross",
            Message::Gesture(Gesture {
                kind: GestureKind::Cross,
                phase: GesturePhase::Recognised,
            }),
        ),
        (
            "capabilitiesCinematic",
            Message::Capabilities(Capabilities {
                controls: ControlCapabilities {
                    gestures: vec![GestureKind::Palm, GestureKind::Cross],
                    cinematic: Some(CinematicCapabilities {
                        positions: vec![Position::Back, Position::Front],
                        aperture: Some(Range {
                            min: 2.0,
                            max: 16.0,
                        }),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
        ),
        (
            "controlCinematic",
            Message::Control(ControlValues {
                cinematic: Some(true),
                aperture: Some(2.5),
                ..Default::default()
            }),
        ),
        (
            "controlCinematicAuto",
            Message::Control(ControlValues {
                cinematic_focus: Some(CinematicFocus::Auto),
                ..Default::default()
            }),
        ),
        (
            "controlCinematicSubject",
            Message::Control(ControlValues {
                cinematic_focus: Some(CinematicFocus::Subject {
                    id: 12,
                    strong: true,
                }),
                ..Default::default()
            }),
        ),
        (
            "controlCinematicTrack",
            Message::Control(ControlValues {
                cinematic_focus: Some(CinematicFocus::Track {
                    point: Point { x: 0.25, y: 0.75 },
                    strong: false,
                }),
                ..Default::default()
            }),
        ),
        (
            "controlCinematicFixed",
            Message::Control(ControlValues {
                cinematic_focus: Some(CinematicFocus::Fixed {
                    point: Point { x: 0.5, y: 0.5 },
                }),
                ..Default::default()
            }),
        ),
        (
            "cinematic",
            Message::Cinematic(CinematicState {
                subjects: vec![
                    CinematicSubject {
                        id: 12,
                        kind: SubjectKind::Face,
                        rect: Rect {
                            x: 0.375,
                            y: 0.25,
                            width: 0.1875,
                            height: 0.25,
                        },
                        focus: SubjectFocus::Weak,
                    },
                    CinematicSubject {
                        id: 13,
                        kind: SubjectKind::Object,
                        rect: Rect {
                            x: 0.625,
                            y: 0.5,
                            width: 0.125,
                            height: 0.125,
                        },
                        focus: SubjectFocus::None,
                    },
                ],
                low_light: false,
            }),
        ),
        (
            "cinematicLowLight",
            Message::Cinematic(CinematicState {
                subjects: Vec::new(),
                low_light: true,
            }),
        ),
    ];

    let encode = |(name, message): &(&str, Message), exact: bool| {
        json!({
            "name": name,
            // Binary messages must match byte for byte. JSON messages only need to decode to the
            // same values, because key order and number formatting may differ between encoders.
            "exact": exact,
            "bytes": hex::encode(message.encode()),
        })
    };
    binary
        .iter()
        .map(|entry| encode(entry, true))
        .chain(json_messages.iter().map(|entry| encode(entry, false)))
        .collect()
}

fn vectors() -> Value {
    let pair = handshake(Mode::Pair, &UNPAIRED, 0x20);
    let resume = handshake(Mode::Resume, &pair.keys.pairing_key, 0x80);
    json!({
        "version": paircam_protocol::VERSION,
        "handshakes": [pair.vector, resume.vector],
        "sealed": sealed_records(&pair.keys.host_to_camera),
        "messages": messages(),
    })
}

/// Rebuilds every object with its keys in alphabetical order. `serde_json` keeps insertion order
/// instead when another crate in the build turns on its `preserve_order` feature, and the file
/// must not depend on that.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn vectors_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/vectors/v0.json")
}

#[test]
fn vectors_file_matches_the_implementation() {
    let expected = serde_json::to_string_pretty(&sorted(vectors())).unwrap() + "\n";
    let path = vectors_path();

    if std::env::var_os("PAIRCAM_UPDATE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &expected).unwrap();
        return;
    }

    let actual = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(
        actual, expected,
        "protocol/vectors/v0.json is out of date; see the comment at the top of this test"
    );
}
