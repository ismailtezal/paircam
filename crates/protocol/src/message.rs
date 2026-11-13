//! The messages carried inside encrypted records. See "Messages" in `docs/PROTOCOL.md`.

use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

mod kind {
    pub const HELLO: u8 = 0x01;
    pub const PAIR_RESULT: u8 = 0x02;
    pub const CAPABILITIES: u8 = 0x03;
    pub const CONFIGURE: u8 = 0x04;
    pub const START_STREAM: u8 = 0x05;
    pub const STOP_STREAM: u8 = 0x06;
    pub const VIDEO_CONFIG: u8 = 0x10;
    pub const VIDEO_FRAME: u8 = 0x11;
    pub const REQUEST_KEYFRAME: u8 = 0x12;
    pub const CONTROL: u8 = 0x20;
    pub const CONTROL_STATE: u8 = 0x21;
    pub const SUBJECT: u8 = 0x22;
    pub const GESTURE: u8 = 0x23;
    pub const CINEMATIC: u8 = 0x24;
    pub const STATS: u8 = 0x30;
    pub const LOG: u8 = 0x31;
    pub const PING: u8 = 0x3E;
    pub const PONG: u8 = 0x3F;
    pub const BYE: u8 = 0x7F;
}

/// One protocol message.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Hello(Hello),
    PairResult(PairResult),
    Capabilities(Capabilities),
    Configure(Configure),
    StartStream,
    StopStream,
    VideoConfig(VideoConfig),
    VideoFrame(VideoFrame),
    RequestKeyframe,
    Control(ControlValues),
    ControlState(ControlValues),
    Subject(Subject),
    Gesture(Gesture),
    Cinematic(CinematicState),
    Stats(Stats),
    Log(LogEntry),
    Ping(u64),
    Pong(u64),
    Bye(Bye),
    /// A message type this version does not know. It is skipped, not treated as an error.
    Unknown(u8),
}

impl Message {
    /// Encodes the message as the plaintext of one record.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Hello(value) => json(kind::HELLO, value),
            Self::PairResult(value) => json(kind::PAIR_RESULT, value),
            Self::Capabilities(value) => json(kind::CAPABILITIES, value),
            Self::Configure(value) => json(kind::CONFIGURE, value),
            Self::StartStream => vec![kind::START_STREAM],
            Self::StopStream => vec![kind::STOP_STREAM],
            Self::VideoConfig(value) => value.encode(),
            Self::VideoFrame(value) => value.encode(),
            Self::RequestKeyframe => vec![kind::REQUEST_KEYFRAME],
            Self::Control(value) => json(kind::CONTROL, value),
            Self::ControlState(value) => json(kind::CONTROL_STATE, value),
            Self::Subject(value) => json(kind::SUBJECT, value),
            Self::Gesture(value) => json(kind::GESTURE, value),
            Self::Cinematic(value) => json(kind::CINEMATIC, value),
            Self::Stats(value) => json(kind::STATS, value),
            Self::Log(value) => json(kind::LOG, value),
            Self::Ping(timestamp) => timestamped(kind::PING, *timestamp),
            Self::Pong(timestamp) => timestamped(kind::PONG, *timestamp),
            Self::Bye(value) => json(kind::BYE, value),
            Self::Unknown(kind) => vec![*kind],
        }
    }

    /// Decodes the plaintext of one record.
    pub fn decode(message: Vec<u8>) -> Result<Self> {
        let message = Bytes::from(message);
        let Some(&kind) = message.first() else {
            return Err(Error::Malformed("empty message"));
        };
        let payload = message.slice(1..);

        Ok(match kind {
            kind::HELLO => Self::Hello(parse("hello", &payload)?),
            kind::PAIR_RESULT => Self::PairResult(parse("pairResult", &payload)?),
            kind::CAPABILITIES => Self::Capabilities(parse("capabilities", &payload)?),
            kind::CONFIGURE => Self::Configure(parse("configure", &payload)?),
            kind::START_STREAM => Self::StartStream,
            kind::STOP_STREAM => Self::StopStream,
            kind::VIDEO_CONFIG => Self::VideoConfig(VideoConfig::decode(payload)?),
            kind::VIDEO_FRAME => Self::VideoFrame(VideoFrame::decode(payload)?),
            kind::REQUEST_KEYFRAME => Self::RequestKeyframe,
            kind::CONTROL => Self::Control(parse("control", &payload)?),
            kind::CONTROL_STATE => Self::ControlState(parse("controlState", &payload)?),
            kind::SUBJECT => Self::Subject(parse("subject", &payload)?),
            kind::GESTURE => Self::Gesture(parse("gesture", &payload)?),
            kind::CINEMATIC => Self::Cinematic(parse("cinematic", &payload)?),
            kind::STATS => Self::Stats(parse("stats", &payload)?),
            kind::LOG => Self::Log(parse("log", &payload)?),
            kind::PING => Self::Ping(timestamp("ping", &payload)?),
            kind::PONG => Self::Pong(timestamp("pong", &payload)?),
            kind::BYE => Self::Bye(parse("bye", &payload)?),
            other => Self::Unknown(other),
        })
    }
}

fn json(kind: u8, value: &impl Serialize) -> Vec<u8> {
    let mut message = vec![kind];
    serde_json::to_writer(&mut message, value).expect("protocol types always serialize");
    message
}

fn parse<T: DeserializeOwned>(message: &'static str, payload: &[u8]) -> Result<T> {
    serde_json::from_slice(payload).map_err(|source| Error::Json { message, source })
}

fn timestamped(kind: u8, timestamp: u64) -> Vec<u8> {
    let mut message = vec![kind];
    message.extend_from_slice(&timestamp.to_be_bytes());
    message
}

fn timestamp(message: &'static str, payload: &[u8]) -> Result<u64> {
    let bytes: [u8; 8] = payload
        .get(..8)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Error::Malformed(message))?;
    Ok(u64::from_be_bytes(bytes))
}

// ---------------------------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    H264,
    Hevc,
}

/// How to decode and present the frames that follow.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoConfig {
    pub codec: Codec,
    /// Encoded picture size in pixels.
    pub width: u16,
    pub height: u16,
    /// Clockwise degrees the host must rotate the picture: 0, 90, 180 or 270.
    pub rotation: u16,
    /// The picture must be mirrored horizontally (front camera).
    pub mirrored: bool,
    /// The decoder configuration record: `avcC` for H.264, `hvcC` for HEVC.
    pub record: Bytes,
}

impl VideoConfig {
    const HEADER_LEN: usize = 1 + 2 + 2 + 2 + 1 + 4;

    fn encode(&self) -> Vec<u8> {
        let mut message = Vec::with_capacity(1 + Self::HEADER_LEN + self.record.len());
        message.push(kind::VIDEO_CONFIG);
        message.push(match self.codec {
            Codec::H264 => 0,
            Codec::Hevc => 1,
        });
        message.extend_from_slice(&self.width.to_be_bytes());
        message.extend_from_slice(&self.height.to_be_bytes());
        message.extend_from_slice(&self.rotation.to_be_bytes());
        message.push(u8::from(self.mirrored));
        message.extend_from_slice(&(self.record.len() as u32).to_be_bytes());
        message.extend_from_slice(&self.record);
        message
    }

    fn decode(payload: Bytes) -> Result<Self> {
        const NAME: &str = "videoConfig";
        if payload.len() < Self::HEADER_LEN {
            return Err(Error::Malformed(NAME));
        }
        let codec = match payload[0] {
            0 => Codec::H264,
            1 => Codec::Hevc,
            _ => return Err(Error::Malformed("videoConfig codec")),
        };
        let u16_at = |offset: usize| u16::from_be_bytes([payload[offset], payload[offset + 1]]);
        let record_len =
            u32::from_be_bytes(payload[8..12].try_into().expect("length checked")) as usize;
        let record_end = Self::HEADER_LEN
            .checked_add(record_len)
            .filter(|end| *end <= payload.len())
            .ok_or(Error::Malformed(NAME))?;

        Ok(Self {
            codec,
            width: u16_at(1),
            height: u16_at(3),
            rotation: u16_at(5),
            mirrored: payload[7] & 1 != 0,
            record: payload.slice(Self::HEADER_LEN..record_end),
        })
    }
}

/// One compressed picture.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFrame {
    /// Capture time in microseconds on the camera's monotonic clock.
    pub timestamp_us: u64,
    pub keyframe: bool,
    /// One access unit: NAL units, each preceded by its `u32` length.
    pub data: Bytes,
}

impl VideoFrame {
    const HEADER_LEN: usize = 8 + 1;

    fn encode(&self) -> Vec<u8> {
        let mut message = Vec::with_capacity(1 + Self::HEADER_LEN + self.data.len());
        message.push(kind::VIDEO_FRAME);
        message.extend_from_slice(&self.timestamp_us.to_be_bytes());
        message.push(u8::from(self.keyframe));
        message.extend_from_slice(&self.data);
        message
    }

    fn decode(payload: Bytes) -> Result<Self> {
        if payload.len() < Self::HEADER_LEN {
            return Err(Error::Malformed("videoFrame"));
        }
        Ok(Self {
            timestamp_us: u64::from_be_bytes(payload[..8].try_into().expect("length checked")),
            keyframe: payload[8] & 1 != 0,
            data: payload.slice(Self::HEADER_LEN..),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// JSON payloads
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Hello {
    /// The device name shown to the user, such as "Ismail's iPhone".
    pub name: String,
    pub app_version: String,
    /// `ios`, `android`, `windows`, `macos` or `linux`.
    pub platform: String,
    /// The hardware model, such as "iPhone16,2". Empty on the desktop.
    pub model: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PairResult {
    pub accepted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub cameras: Vec<CameraInfo>,
    pub codecs: Vec<Codec>,
    pub controls: ControlCapabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
    pub position: Position,
    pub lens: Lens,
    pub formats: Vec<Format>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Position {
    Front,
    Back,
    External,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lens {
    Wide,
    UltraWide,
    Telephoto,
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Format {
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub min: f32,
    pub max: f32,
}

/// Which controls the camera supports. An absent or empty entry means "not supported".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ControlCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: Option<Range>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure_bias: Option<Range>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub focus: Vec<FocusMode>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub white_balance: Vec<WhiteBalanceMode>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub torch: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stabilization: Vec<Stabilization>,
    /// The camera can find the person and report them in [`Subject`] messages.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub face_tracking: bool,
    /// The hand gestures the camera can recognise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gestures: Vec<GestureKind>,
    /// Cinematic mode: the camera blurs the background like a film lens and racks focus
    /// between subjects. Absent when the phone cannot do it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cinematic: Option<CinematicCapabilities>,
}

/// Where and how the camera can film in Cinematic mode.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CinematicCapabilities {
    /// The sides of the phone with a camera that can do it. With Cinematic mode on, the phone
    /// films from that camera on the side of the camera the host chose.
    pub positions: Vec<Position>,
    /// The simulated aperture range as f-numbers: the smaller, the stronger the blur.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<Range>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FocusMode {
    Auto,
    Locked,
    Manual,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WhiteBalanceMode {
    Auto,
    Locked,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stabilization {
    Off,
    Standard,
    Cinematic,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configure {
    /// The `id` of one of the cameras in [`Capabilities`].
    pub camera: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    /// Bits per second.
    pub bitrate: u32,
    /// Seconds between keyframes.
    pub keyframe_interval: f32,
}

/// A point in the picture, with both axes from 0 to 1 and the origin at the top left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// Camera control values. In `control` only the fields being changed are present; in
/// `controlState` every supported one is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ControlValues {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusMode>,
    /// Lens position for manual focus, from 0 (near) to 1 (far).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_position: Option<f32>,
    /// Focus and expose on this point once, as a tap on the preview does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_point: Option<Point>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure_bias: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white_balance: Option<WhiteBalanceMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub torch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stabilization: Option<Stabilization>,
    /// Whether the camera reports where the person is, in [`Subject`] messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_tracking: Option<bool>,
    /// Whether the camera watches for hand gestures and acts on them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gestures: Option<bool>,
    /// Whether Cinematic mode is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cinematic: Option<bool>,
    /// The simulated aperture in Cinematic mode, as an f-number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<f32>,
    /// Moves the focus in Cinematic mode once, as a tap or a long press on the preview does.
    /// Only in `control`; where the focus is shows in [`CinematicState`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cinematic_focus: Option<CinematicFocus>,
}

/// Where Cinematic mode should focus.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum CinematicFocus {
    /// The camera decides again, racking focus as subjects come, go and turn.
    Auto,
    /// Focus on a subject from [`CinematicState`]. A strong focus stays on it until it leaves
    /// the picture; a weak one lets the camera rack away when another subject matters more.
    Subject {
        id: i64,
        #[serde(default)]
        strong: bool,
    },
    /// Find whatever is at the point and follow it, as a tap does.
    Track {
        point: Point,
        #[serde(default)]
        strong: bool,
    },
    /// Hold the focus at the distance of whatever is at the point, as a long press does.
    Fixed { point: Point },
    /// A mode this version does not know. Ignored.
    #[serde(other)]
    Unknown,
}

/// What Cinematic mode sees and where it focuses, sent while it is on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CinematicState {
    /// Everything the camera could focus on, in the encoded picture. At most one is in focus.
    pub subjects: Vec<CinematicSubject>,
    /// There is not enough light for Cinematic mode to work well.
    pub low_light: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CinematicSubject {
    /// Stays the same while the camera keeps seeing the subject. Use it in
    /// [`CinematicFocus::Subject`].
    pub id: i64,
    pub kind: SubjectKind,
    /// Where it is in the encoded picture, with both axes from 0 to 1 and the origin at the
    /// top left.
    pub rect: Rect,
    pub focus: SubjectFocus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubjectKind {
    Face,
    Head,
    Body,
    Hand,
    Animal,
    /// Anything else that stands out, such as an object held up.
    Object,
    /// A fixed focus distance the user chose, rather than something followed.
    Fixed,
    #[default]
    #[serde(other)]
    Unknown,
}

/// Whether a subject is in focus, and how firmly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubjectFocus {
    #[default]
    None,
    /// In focus until the camera finds a better subject.
    Weak,
    /// In focus until it leaves the picture.
    Strong,
    #[serde(other)]
    Unknown,
}

/// A rectangle in the picture, with both axes from 0 to 1 and the origin at the top left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Where the person is in the encoded picture, while face tracking is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subject {
    /// The face, or `None` when nobody is in view. Always written, as `null` when empty.
    pub face: Option<Rect>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GestureKind {
    /// An open palm held up: in Cinematic mode, pulls focus to the hand and back; otherwise
    /// toggles face tracking.
    Palm,
    /// Thumb and index finger in an L shape: zooms in one step.
    Frame,
    /// Two index fingers crossed in an X: zooms out one step.
    Cross,
    /// Two hands moving apart or together: zooms continuously.
    Spread,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GesturePhase {
    /// A one-off gesture was recognised and acted on.
    #[default]
    Recognised,
    /// A continuous gesture started.
    Began,
    /// A continuous gesture ended.
    Ended,
    #[serde(other)]
    Unknown,
}

/// A hand gesture the camera recognised and acted on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gesture {
    pub kind: GestureKind,
    pub phase: GesturePhase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Thermal {
    #[default]
    Nominal,
    Fair,
    Serious,
    Critical,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Stats {
    pub fps: f32,
    /// Bits per second actually produced by the encoder.
    pub bitrate: u32,
    /// Frames dropped since the stream started because the connection fell behind.
    pub dropped_frames: u32,
    pub thermal: Thermal,
    /// Battery level from 0 to 1.
    pub battery: f32,
    pub charging: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warning,
    Error,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogEntry {
    pub level: LogLevel,
    pub tag: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bye {
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(message: Message) {
        let decoded = Message::decode(message.encode()).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_every_message() {
        round_trip(Message::Hello(Hello {
            name: "Ismail's iPhone".into(),
            app_version: "0.1.0".into(),
            platform: "ios".into(),
            model: "iPhone16,2".into(),
        }));
        round_trip(Message::PairResult(PairResult { accepted: true }));
        round_trip(Message::Capabilities(Capabilities {
            cameras: vec![CameraInfo {
                id: "back-wide".into(),
                name: "Back Camera".into(),
                position: Position::Back,
                lens: Lens::Wide,
                formats: vec![Format {
                    width: 1920,
                    height: 1080,
                    max_fps: 60,
                }],
            }],
            codecs: vec![Codec::Hevc, Codec::H264],
            controls: ControlCapabilities {
                zoom: Some(Range {
                    min: 1.0,
                    max: 10.0,
                }),
                focus: vec![FocusMode::Auto, FocusMode::Locked],
                torch: true,
                face_tracking: true,
                gestures: vec![GestureKind::Palm, GestureKind::Frame, GestureKind::Spread],
                ..Default::default()
            },
        }));
        round_trip(Message::Configure(Configure {
            camera: "back-wide".into(),
            width: 1920,
            height: 1080,
            fps: 30,
            codec: Codec::Hevc,
            bitrate: 12_000_000,
            keyframe_interval: 2.0,
        }));
        round_trip(Message::StartStream);
        round_trip(Message::StopStream);
        round_trip(Message::VideoConfig(VideoConfig {
            codec: Codec::Hevc,
            width: 1920,
            height: 1080,
            rotation: 90,
            mirrored: true,
            record: Bytes::from_static(&[1, 2, 3, 4]),
        }));
        round_trip(Message::VideoFrame(VideoFrame {
            timestamp_us: 1_234_567,
            keyframe: true,
            data: Bytes::from_static(&[0, 0, 0, 1, 0x65]),
        }));
        round_trip(Message::RequestKeyframe);
        round_trip(Message::Control(ControlValues {
            zoom: Some(2.0),
            ..Default::default()
        }));
        round_trip(Message::ControlState(ControlValues {
            zoom: Some(1.0),
            focus: Some(FocusMode::Auto),
            exposure_bias: Some(0.0),
            torch: Some(false),
            face_tracking: Some(true),
            gestures: Some(false),
            ..Default::default()
        }));
        round_trip(Message::Subject(Subject {
            face: Some(Rect {
                x: 0.41,
                y: 0.22,
                width: 0.18,
                height: 0.24,
            }),
        }));
        round_trip(Message::Subject(Subject { face: None }));
        round_trip(Message::Gesture(Gesture {
            kind: GestureKind::Spread,
            phase: GesturePhase::Began,
        }));
        round_trip(Message::Stats(Stats {
            fps: 29.97,
            bitrate: 11_840_000,
            dropped_frames: 2,
            thermal: Thermal::Fair,
            battery: 0.82,
            charging: true,
        }));
        round_trip(Message::Log(LogEntry {
            level: LogLevel::Warning,
            tag: "encoder".into(),
            message: "dropped a frame".into(),
        }));
        round_trip(Message::Ping(42));
        round_trip(Message::Pong(42));
        round_trip(Message::Bye(Bye {
            reason: "closed by user".into(),
        }));
    }

    #[test]
    fn control_only_carries_the_changed_fields() {
        let message = Message::Control(ControlValues {
            torch: Some(true),
            ..Default::default()
        })
        .encode();
        assert_eq!(&message[1..], br#"{"torch":true}"#);
    }

    #[test]
    fn skips_unknown_message_types() {
        assert_eq!(
            Message::decode(vec![0x6A, 1, 2, 3]).unwrap(),
            Message::Unknown(0x6A)
        );
    }

    #[test]
    fn tolerates_unknown_fields_and_values() {
        let mut message = vec![kind::STATS];
        message.extend_from_slice(br#"{"fps":30,"thermal":"melting","newField":[1,2]}"#);
        let Message::Stats(stats) = Message::decode(message).unwrap() else {
            panic!("expected stats");
        };
        assert_eq!(stats.fps, 30.0);
        assert_eq!(stats.thermal, Thermal::Unknown);

        let mut message = vec![kind::GESTURE];
        message.extend_from_slice(br#"{"kind":"wave","phase":"halfway","hand":"left"}"#);
        assert_eq!(
            Message::decode(message).unwrap(),
            Message::Gesture(Gesture {
                kind: GestureKind::Unknown,
                phase: GesturePhase::Unknown,
            })
        );

        let mut message = vec![kind::CAPABILITIES];
        message.extend_from_slice(
            br#"{"controls":{"faceTracking":true,"gestures":["palm","snap","spread"]}}"#,
        );
        let Message::Capabilities(capabilities) = Message::decode(message).unwrap() else {
            panic!("expected capabilities");
        };
        assert!(capabilities.controls.face_tracking);
        assert_eq!(
            capabilities.controls.gestures,
            vec![GestureKind::Palm, GestureKind::Unknown, GestureKind::Spread]
        );
    }

    #[test]
    fn subject_writes_an_empty_face_as_null() {
        let message = Message::Subject(Subject { face: None }).encode();
        assert_eq!(message[0], 0x22);
        assert_eq!(&message[1..], br#"{"face":null}"#);

        let mut message = vec![kind::SUBJECT];
        message.extend_from_slice(br#"{"face":{"x":0.5,"y":0.25,"width":0.1,"height":0.2}}"#);
        let Message::Subject(subject) = Message::decode(message).unwrap() else {
            panic!("expected subject");
        };
        assert_eq!(
            subject.face,
            Some(Rect {
                x: 0.5,
                y: 0.25,
                width: 0.1,
                height: 0.2,
            })
        );
    }

    #[test]
    fn tracking_controls_use_the_specified_names() {
        let message = Message::Control(ControlValues {
            face_tracking: Some(true),
            ..Default::default()
        })
        .encode();
        assert_eq!(message[0], 0x20);
        assert_eq!(&message[1..], br#"{"faceTracking":true}"#);

        let message = Message::Gesture(Gesture {
            kind: GestureKind::Palm,
            phase: GesturePhase::Recognised,
        })
        .encode();
        assert_eq!(message[0], 0x23);
        assert_eq!(&message[1..], br#"{"kind":"palm","phase":"recognised"}"#);
    }

    #[test]
    fn cinematic_messages_round_trip_and_use_the_specified_names() {
        round_trip(Message::Capabilities(Capabilities {
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
        }));
        round_trip(Message::Cinematic(CinematicState {
            subjects: vec![
                CinematicSubject {
                    id: 7,
                    kind: SubjectKind::Face,
                    rect: Rect {
                        x: 0.4,
                        y: 0.2,
                        width: 0.2,
                        height: 0.3,
                    },
                    focus: SubjectFocus::Strong,
                },
                CinematicSubject {
                    id: 9,
                    kind: SubjectKind::Object,
                    rect: Rect::default(),
                    focus: SubjectFocus::None,
                },
            ],
            low_light: true,
        }));
        for focus in [
            CinematicFocus::Auto,
            CinematicFocus::Subject {
                id: 7,
                strong: true,
            },
            CinematicFocus::Track {
                point: Point { x: 0.5, y: 0.25 },
                strong: false,
            },
            CinematicFocus::Fixed {
                point: Point { x: 0.1, y: 0.9 },
            },
        ] {
            round_trip(Message::Control(ControlValues {
                cinematic_focus: Some(focus),
                ..Default::default()
            }));
        }

        let message = Message::Control(ControlValues {
            cinematic: Some(true),
            aperture: Some(2.8),
            cinematic_focus: Some(CinematicFocus::Subject {
                id: 7,
                strong: true,
            }),
            ..Default::default()
        })
        .encode();
        assert_eq!(
            &message[1..],
            br#"{"cinematic":true,"aperture":2.8,"cinematicFocus":{"mode":"subject","id":7,"strong":true}}"#
        );
        let message = Message::Cinematic(CinematicState::default()).encode();
        assert_eq!(message[0], 0x24);
        assert_eq!(&message[1..], br#"{"subjects":[],"lowLight":false}"#);
    }

    #[test]
    fn cinematic_tolerates_unknown_modes_kinds_and_fields() {
        let mut message = vec![kind::CONTROL];
        message.extend_from_slice(br#"{"cinematicFocus":{"mode":"telepathy","target":3}}"#);
        assert_eq!(
            Message::decode(message).unwrap(),
            Message::Control(ControlValues {
                cinematic_focus: Some(CinematicFocus::Unknown),
                ..Default::default()
            })
        );

        let mut message = vec![kind::CONTROL];
        message
            .extend_from_slice(br#"{"cinematicFocus":{"mode":"track","point":{"x":0.5,"y":0.5}}}"#);
        assert_eq!(
            Message::decode(message).unwrap(),
            Message::Control(ControlValues {
                cinematic_focus: Some(CinematicFocus::Track {
                    point: Point { x: 0.5, y: 0.5 },
                    strong: false,
                }),
                ..Default::default()
            })
        );

        let mut message = vec![kind::CINEMATIC];
        message.extend_from_slice(
            br#"{"subjects":[{"id":3,"kind":"robot","rect":{"x":0.1,"y":0.1,"width":0.2,"height":0.2},"focus":"blurry","depth":2}],"future":1}"#,
        );
        let Message::Cinematic(state) = Message::decode(message).unwrap() else {
            panic!("expected cinematic");
        };
        assert_eq!(state.subjects.len(), 1);
        assert_eq!(state.subjects[0].id, 3);
        assert_eq!(state.subjects[0].kind, SubjectKind::Unknown);
        assert_eq!(state.subjects[0].focus, SubjectFocus::Unknown);
        assert!(!state.low_light);
    }

    #[test]
    fn rejects_truncated_binary_messages() {
        assert!(Message::decode(vec![]).is_err());
        assert!(Message::decode(vec![kind::VIDEO_FRAME, 0, 0]).is_err());
        assert!(Message::decode(vec![kind::PING, 0]).is_err());

        let mut config = Message::VideoConfig(VideoConfig {
            codec: Codec::H264,
            width: 1280,
            height: 720,
            rotation: 0,
            mirrored: false,
            record: Bytes::from_static(&[9; 8]),
        })
        .encode();
        config.truncate(config.len() - 1);
        assert!(Message::decode(config).is_err());
    }
}
