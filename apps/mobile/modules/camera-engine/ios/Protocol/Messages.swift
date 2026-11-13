// The messages carried inside encrypted records. See "Messages" in `docs/PROTOCOL.md`; this
// mirrors `crates/protocol/src/message.rs`.
//
// JSON payloads tolerate unknown fields, missing fields and unknown enum values, so newer apps can
// add all three without breaking older ones.

import Foundation

enum MessageKind {
    static let hello: UInt8 = 0x01
    static let pairResult: UInt8 = 0x02
    static let capabilities: UInt8 = 0x03
    static let configure: UInt8 = 0x04
    static let startStream: UInt8 = 0x05
    static let stopStream: UInt8 = 0x06
    static let videoConfig: UInt8 = 0x10
    static let videoFrame: UInt8 = 0x11
    static let requestKeyframe: UInt8 = 0x12
    static let control: UInt8 = 0x20
    static let controlState: UInt8 = 0x21
    static let subject: UInt8 = 0x22
    static let gesture: UInt8 = 0x23
    static let cinematic: UInt8 = 0x24
    static let stats: UInt8 = 0x30
    static let log: UInt8 = 0x31
    static let ping: UInt8 = 0x3E
    static let pong: UInt8 = 0x3F
    static let bye: UInt8 = 0x7F
}

/// One protocol message.
enum Message: Equatable {
    case hello(Hello)
    case pairResult(PairResult)
    case capabilities(Capabilities)
    case configure(Configure)
    case startStream
    case stopStream
    case videoConfig(VideoConfig)
    case videoFrame(VideoFrame)
    case requestKeyframe
    case control(ControlValues)
    case controlState(ControlValues)
    case subject(SubjectUpdate)
    case gesture(GestureEvent)
    case cinematic(CinematicState)
    case stats(Stats)
    case log(LogEntry)
    case ping(UInt64)
    case pong(UInt64)
    case bye(Bye)
    /// A message type this version does not know. It is skipped, not treated as an error.
    case unknown(UInt8)

    /// Encodes the message as the plaintext of one record.
    func encode() throws -> Data {
        switch self {
        case .hello(let value):
            return try Message.json(MessageKind.hello, value)
        case .pairResult(let value):
            return try Message.json(MessageKind.pairResult, value)
        case .capabilities(let value):
            return try Message.json(MessageKind.capabilities, value)
        case .configure(let value):
            return try Message.json(MessageKind.configure, value)
        case .startStream:
            return Data([MessageKind.startStream])
        case .stopStream:
            return Data([MessageKind.stopStream])
        case .videoConfig(let value):
            return value.encode()
        case .videoFrame(let value):
            return value.encode()
        case .requestKeyframe:
            return Data([MessageKind.requestKeyframe])
        case .control(let value):
            return try Message.json(MessageKind.control, value)
        case .controlState(let value):
            return try Message.json(MessageKind.controlState, value)
        case .subject(let value):
            return try Message.json(MessageKind.subject, value)
        case .gesture(let value):
            return try Message.json(MessageKind.gesture, value)
        case .cinematic(let value):
            return try Message.json(MessageKind.cinematic, value)
        case .stats(let value):
            return try Message.json(MessageKind.stats, value)
        case .log(let value):
            return try Message.json(MessageKind.log, value)
        case .ping(let timestamp):
            return Message.timestamped(MessageKind.ping, timestamp)
        case .pong(let timestamp):
            return Message.timestamped(MessageKind.pong, timestamp)
        case .bye(let value):
            return try Message.json(MessageKind.bye, value)
        case .unknown(let kind):
            return Data([kind])
        }
    }

    /// Decodes the plaintext of one record.
    static func decode(_ message: Data) throws -> Message {
        var reader = ByteReader(message)
        guard let kind = reader.readUInt8() else {
            throw ProtocolError.malformed("empty message")
        }
        let payload = reader.readRemaining()

        switch kind {
        case MessageKind.hello:
            return .hello(try parse("hello", payload))
        case MessageKind.pairResult:
            return .pairResult(try parse("pairResult", payload))
        case MessageKind.capabilities:
            return .capabilities(try parse("capabilities", payload))
        case MessageKind.configure:
            return .configure(try parse("configure", payload))
        case MessageKind.startStream:
            return .startStream
        case MessageKind.stopStream:
            return .stopStream
        case MessageKind.videoConfig:
            return .videoConfig(try VideoConfig(payload: payload))
        case MessageKind.videoFrame:
            return .videoFrame(try VideoFrame(payload: payload))
        case MessageKind.requestKeyframe:
            return .requestKeyframe
        case MessageKind.control:
            return .control(try parse("control", payload))
        case MessageKind.controlState:
            return .controlState(try parse("controlState", payload))
        case MessageKind.subject:
            return .subject(try parse("subject", payload))
        case MessageKind.gesture:
            return .gesture(try parse("gesture", payload))
        case MessageKind.cinematic:
            return .cinematic(try parse("cinematic", payload))
        case MessageKind.stats:
            return .stats(try parse("stats", payload))
        case MessageKind.log:
            return .log(try parse("log", payload))
        case MessageKind.ping:
            return .ping(try timestamp("ping", payload))
        case MessageKind.pong:
            return .pong(try timestamp("pong", payload))
        case MessageKind.bye:
            return .bye(try parse("bye", payload))
        default:
            return .unknown(kind)
        }
    }

    private static func json<T: Encodable>(_ kind: UInt8, _ value: T) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.withoutEscapingSlashes]
        do {
            var message = Data([kind])
            message.append(try encoder.encode(value))
            return message
        } catch {
            throw ProtocolError.json(String(describing: error))
        }
    }

    private static func parse<T: Decodable>(_ name: String, _ payload: Data) throws -> T {
        do {
            return try JSONDecoder().decode(T.self, from: payload)
        } catch {
            throw ProtocolError.json(name)
        }
    }

    private static func timestamped(_ kind: UInt8, _ timestamp: UInt64) -> Data {
        var message = Data([kind])
        message.appendBigEndian(timestamp)
        return message
    }

    private static func timestamp(_ name: String, _ payload: Data) throws -> UInt64 {
        var reader = ByteReader(payload)
        guard let value = reader.readUInt64() else {
            throw ProtocolError.malformed(name)
        }
        return value
    }
}

// MARK: - Video

enum VideoCodec: String, Codable, Equatable {
    case h264
    case hevc

    var wireValue: UInt8 {
        switch self {
        case .h264:
            return 0
        case .hevc:
            return 1
        }
    }
}

/// How to decode and present the frames that follow.
struct VideoConfig: Equatable {
    static let headerLength = 1 + 2 + 2 + 2 + 1 + 4

    var codec: VideoCodec
    /// Encoded picture size in pixels.
    var width: UInt16
    var height: UInt16
    /// Clockwise degrees the host must rotate the picture: 0, 90, 180 or 270.
    var rotation: UInt16
    /// The picture must be mirrored horizontally.
    var mirrored: Bool
    /// The decoder configuration record: `avcC` for H.264, `hvcC` for HEVC.
    var record: Data

    func encode() -> Data {
        var message = Data(capacity: 1 + VideoConfig.headerLength + record.count)
        message.append(MessageKind.videoConfig)
        message.append(codec.wireValue)
        message.appendBigEndian(width)
        message.appendBigEndian(height)
        message.appendBigEndian(rotation)
        message.append(mirrored ? 1 : 0)
        message.appendBigEndian(UInt32(record.count))
        message.append(record)
        return message
    }
}

extension VideoConfig {
    init(payload: Data) throws {
        var reader = ByteReader(payload)
        guard
            reader.remaining >= VideoConfig.headerLength,
            let codecByte = reader.readUInt8(),
            let width = reader.readUInt16(),
            let height = reader.readUInt16(),
            let rotation = reader.readUInt16(),
            let flags = reader.readUInt8(),
            let recordLength = reader.readUInt32()
        else {
            throw ProtocolError.malformed("videoConfig")
        }
        let codec: VideoCodec
        switch codecByte {
        case 0:
            codec = .h264
        case 1:
            codec = .hevc
        default:
            throw ProtocolError.malformed("videoConfig codec")
        }
        guard let record = reader.readBytes(Int(recordLength)) else {
            throw ProtocolError.malformed("videoConfig")
        }
        self.init(
            codec: codec,
            width: width,
            height: height,
            rotation: rotation,
            mirrored: flags & 1 != 0,
            record: record
        )
    }
}

/// One compressed picture.
struct VideoFrame: Equatable {
    static let headerLength = 8 + 1

    /// Capture time in microseconds on the camera's monotonic clock.
    var timestampMicros: UInt64
    var keyframe: Bool
    /// One access unit: NAL units, each preceded by its `u32` length.
    var data: Data

    func encode() -> Data {
        var message = Data(capacity: 1 + VideoFrame.headerLength + data.count)
        message.append(MessageKind.videoFrame)
        message.appendBigEndian(timestampMicros)
        message.append(keyframe ? 1 : 0)
        message.append(data)
        return message
    }
}

extension VideoFrame {
    init(payload: Data) throws {
        var reader = ByteReader(payload)
        guard
            reader.remaining >= VideoFrame.headerLength,
            let timestamp = reader.readUInt64(),
            let flags = reader.readUInt8()
        else {
            throw ProtocolError.malformed("videoFrame")
        }
        self.init(timestampMicros: timestamp, keyframe: flags & 1 != 0, data: reader.readRemaining())
    }
}

// MARK: - JSON payloads

/// Decodes a string enum, mapping values this version does not know to `fallback`.
private func decodeTolerant<T: RawRepresentable>(_ decoder: Decoder, fallback: T) throws -> T
where T.RawValue == String {
    let raw = try decoder.singleValueContainer().decode(String.self)
    return T(rawValue: raw) ?? fallback
}

struct Hello: Codable, Equatable {
    /// The device name shown to the user.
    var name: String = ""
    var appVersion: String = ""
    /// `ios`, `android`, `windows`, `macos` or `linux`.
    var platform: String = ""
    /// The hardware model, such as "iPhone16,2". Empty on the desktop.
    var model: String = ""

    private enum CodingKeys: String, CodingKey {
        case name
        case appVersion
        case platform
        case model
    }
}

extension Hello {
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            name: try container.decodeIfPresent(String.self, forKey: .name) ?? "",
            appVersion: try container.decodeIfPresent(String.self, forKey: .appVersion) ?? "",
            platform: try container.decodeIfPresent(String.self, forKey: .platform) ?? "",
            model: try container.decodeIfPresent(String.self, forKey: .model) ?? ""
        )
    }
}

struct PairResult: Codable, Equatable {
    var accepted: Bool
}

struct Capabilities: Codable, Equatable {
    var cameras: [CameraInfo] = []
    var codecs: [VideoCodec] = []
    var controls = ControlCapabilities()

    private enum CodingKeys: String, CodingKey {
        case cameras
        case codecs
        case controls
    }
}

extension Capabilities {
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            cameras: try container.decodeIfPresent([CameraInfo].self, forKey: .cameras) ?? [],
            codecs: try container.decodeIfPresent([VideoCodec].self, forKey: .codecs) ?? [],
            controls: try container.decodeIfPresent(ControlCapabilities.self, forKey: .controls)
                ?? ControlCapabilities()
        )
    }
}

struct CameraInfo: Codable, Equatable {
    var id: String
    var name: String
    var position: CameraPosition
    var lens: CameraLens
    var formats: [VideoFormat]
}

enum CameraPosition: String, Codable, Equatable {
    case front
    case back
    case external
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

enum CameraLens: String, Codable, Equatable {
    case wide
    case ultraWide
    case telephoto
    case other

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .other)
    }
}

struct VideoFormat: Codable, Equatable, Hashable {
    var width: Int
    var height: Int
    var maxFps: Int
}

struct ValueRange: Codable, Equatable {
    var min: Double
    var max: Double
}

/// Which controls the camera supports. An absent or empty entry means "not supported".
struct ControlCapabilities: Equatable {
    var zoom: ValueRange?
    var exposureBias: ValueRange?
    var focus: [FocusMode] = []
    var whiteBalance: [WhiteBalanceMode] = []
    var torch = false
    var stabilization: [StabilizationMode] = []
    /// The camera can find the person and send `subject`.
    var faceTracking = false
    /// The hand gestures the camera recognises.
    var gestures: [GestureKind] = []
    /// Cinematic mode, when the phone can do it.
    var cinematic: CinematicCapabilities? = nil
}

extension ControlCapabilities: Codable {
    private enum CodingKeys: String, CodingKey {
        case zoom
        case exposureBias
        case focus
        case whiteBalance
        case torch
        case stabilization
        case faceTracking
        case gestures
        case cinematic
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            zoom: try container.decodeIfPresent(ValueRange.self, forKey: .zoom),
            exposureBias: try container.decodeIfPresent(ValueRange.self, forKey: .exposureBias),
            focus: try container.decodeIfPresent([FocusMode].self, forKey: .focus) ?? [],
            whiteBalance: try container.decodeIfPresent([WhiteBalanceMode].self, forKey: .whiteBalance) ?? [],
            torch: try container.decodeIfPresent(Bool.self, forKey: .torch) ?? false,
            stabilization: try container.decodeIfPresent([StabilizationMode].self, forKey: .stabilization) ?? [],
            faceTracking: try container.decodeIfPresent(Bool.self, forKey: .faceTracking) ?? false,
            gestures: try container.decodeIfPresent([GestureKind].self, forKey: .gestures) ?? [],
            cinematic: try container.decodeIfPresent(CinematicCapabilities.self, forKey: .cinematic)
        )
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(zoom, forKey: .zoom)
        try container.encodeIfPresent(exposureBias, forKey: .exposureBias)
        if !focus.isEmpty {
            try container.encode(focus, forKey: .focus)
        }
        if !whiteBalance.isEmpty {
            try container.encode(whiteBalance, forKey: .whiteBalance)
        }
        if torch {
            try container.encode(torch, forKey: .torch)
        }
        if !stabilization.isEmpty {
            try container.encode(stabilization, forKey: .stabilization)
        }
        if faceTracking {
            try container.encode(faceTracking, forKey: .faceTracking)
        }
        if !gestures.isEmpty {
            try container.encode(gestures, forKey: .gestures)
        }
        try container.encodeIfPresent(cinematic, forKey: .cinematic)
    }
}

enum FocusMode: String, Codable, Equatable {
    case auto
    case locked
    case manual
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

enum WhiteBalanceMode: String, Codable, Equatable {
    case auto
    case locked
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

enum StabilizationMode: String, Codable, Equatable {
    case off
    case standard
    case cinematic
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

struct Configure: Codable, Equatable {
    /// The `id` of one of the cameras in `Capabilities`.
    var camera: String
    var width: Int
    var height: Int
    var fps: Int
    var codec: VideoCodec
    /// Bits per second.
    var bitrate: Int
    /// Seconds between keyframes.
    var keyframeInterval: Double
}

/// A point in the picture, with both axes from 0 to 1 and the origin at the top left.
struct PicturePoint: Codable, Equatable {
    var x: Double
    var y: Double
}

/// Camera control values. In `control` only the fields being changed are present; in
/// `controlState` every supported one is.
struct ControlValues: Codable, Equatable {
    var zoom: Double?
    var focus: FocusMode?
    /// Lens position for manual focus, from 0 (near) to 1 (far).
    var focusPosition: Double?
    /// Focus and expose on this point once, as a tap on the preview does.
    var focusPoint: PicturePoint?
    var exposureBias: Double?
    var whiteBalance: WhiteBalanceMode?
    var torch: Bool?
    var stabilization: StabilizationMode?
    /// The camera sends `subject` while this is on.
    var faceTracking: Bool?
    /// The camera watches for hand gestures while this is on.
    var gestures: Bool?
    /// Whether Cinematic mode is on.
    var cinematic: Bool?
    /// The simulated aperture in Cinematic mode, as an f-number.
    var aperture: Double?
    /// Moves the focus in Cinematic mode once. Only in `control`.
    var cinematicFocus: CinematicFocus?
}

// MARK: - Face tracking and hand gestures

/// A rectangle in the encoded picture, with the origin at the top left and both axes from 0 to 1.
struct PictureRect: Codable, Equatable {
    var x: Double
    var y: Double
    var width: Double
    var height: Double
}

/// Where the person is in the picture. `face` is `nil` when nobody is in view, and is then
/// written as an explicit `null`.
struct SubjectUpdate: Equatable {
    var face: PictureRect?
}

extension SubjectUpdate: Codable {
    private enum CodingKeys: String, CodingKey {
        case face
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(face: try container.decodeIfPresent(PictureRect.self, forKey: .face))
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        if let face {
            try container.encode(face, forKey: .face)
        } else {
            try container.encodeNil(forKey: .face)
        }
    }
}

enum GestureKind: String, Codable, Equatable {
    /// An open palm held up for about a second: in Cinematic mode pulls focus to the hand and
    /// back, otherwise toggles face tracking.
    case palm
    /// Thumb and index finger in an L shape, held for about a second: zooms in one step.
    case frame
    /// Two index fingers crossed in an X, held for about a second: zooms out one step.
    case cross
    /// Two hands moving apart or together: zooms continuously.
    case spread
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

enum GesturePhase: String, Codable, Equatable {
    /// A one-off gesture fired.
    case recognised
    /// A continuous gesture started.
    case began
    /// A continuous gesture finished.
    case ended
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

struct GestureEvent: Codable, Equatable {
    var kind: GestureKind
    var phase: GesturePhase
}

// MARK: - Cinematic mode

/// Where and how the camera can film in Cinematic mode.
struct CinematicCapabilities: Equatable {
    /// The sides of the phone with a camera that can do it.
    var positions: [CameraPosition] = []
    /// The simulated aperture range as f-numbers: the smaller, the stronger the blur.
    var aperture: ValueRange? = nil
}

extension CinematicCapabilities: Codable {
    private enum CodingKeys: String, CodingKey {
        case positions
        case aperture
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            positions: try container.decodeIfPresent([CameraPosition].self, forKey: .positions) ?? [],
            aperture: try container.decodeIfPresent(ValueRange.self, forKey: .aperture)
        )
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(positions, forKey: .positions)
        try container.encodeIfPresent(aperture, forKey: .aperture)
    }
}

/// Where Cinematic mode should focus. Written as an object whose `mode` says which case it is.
enum CinematicFocus: Equatable {
    /// The camera decides again, racking focus as subjects come, go and turn.
    case auto
    /// Focus on a subject from `CinematicState`. A strong focus stays until it leaves the
    /// picture; a weak one lets the camera rack away when another subject matters more.
    case subject(id: Int, strong: Bool)
    /// Find whatever is at the point and follow it, as a tap does.
    case track(point: PicturePoint, strong: Bool)
    /// Hold the focus at the distance of whatever is at the point, as a long press does.
    case fixed(point: PicturePoint)
    /// A mode this version does not know. Ignored.
    case unknown
}

extension CinematicFocus: Codable {
    private enum CodingKeys: String, CodingKey {
        case mode
        case id
        case strong
        case point
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let mode = (try? container.decodeIfPresent(String.self, forKey: .mode)) ?? nil
        let strong = ((try? container.decodeIfPresent(Bool.self, forKey: .strong)) ?? nil) ?? false
        let point = (try? container.decodeIfPresent(PicturePoint.self, forKey: .point)) ?? nil
        switch mode {
        case "auto":
            self = .auto
        case "subject":
            if let id = (try? container.decodeIfPresent(Int.self, forKey: .id)) ?? nil {
                self = .subject(id: id, strong: strong)
            } else {
                self = .unknown
            }
        case "track":
            self = point.map { .track(point: $0, strong: strong) } ?? .unknown
        case "fixed":
            self = point.map { .fixed(point: $0) } ?? .unknown
        default:
            self = .unknown
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .auto:
            try container.encode("auto", forKey: .mode)
        case .subject(let id, let strong):
            try container.encode("subject", forKey: .mode)
            try container.encode(id, forKey: .id)
            try container.encode(strong, forKey: .strong)
        case .track(let point, let strong):
            try container.encode("track", forKey: .mode)
            try container.encode(point, forKey: .point)
            try container.encode(strong, forKey: .strong)
        case .fixed(let point):
            try container.encode("fixed", forKey: .mode)
            try container.encode(point, forKey: .point)
        case .unknown:
            try container.encode("unknown", forKey: .mode)
        }
    }
}

enum SubjectKind: String, Codable, Equatable {
    case face
    case head
    case body
    case hand
    case animal
    /// Anything else that stands out, such as an object held up.
    case object
    /// A fixed focus distance the user chose, rather than something followed.
    case fixed
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

/// Whether a subject is in focus, and how firmly.
enum SubjectFocus: String, Codable, Equatable {
    case unfocused = "none"
    /// In focus until the camera finds a better subject.
    case weak
    /// In focus until it leaves the picture.
    case strong
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

/// Something Cinematic mode could focus on.
struct CinematicSubject: Equatable {
    /// Stays the same while the camera keeps seeing the subject.
    var id: Int
    var kind: SubjectKind
    /// Where it is in the encoded picture, with the origin at the top left and both axes from
    /// 0 to 1.
    var rect: PictureRect
    var focus: SubjectFocus
}

extension CinematicSubject: Codable {
    private enum CodingKeys: String, CodingKey {
        case id
        case kind
        case rect
        case focus
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            id: try container.decodeIfPresent(Int.self, forKey: .id) ?? 0,
            kind: try container.decodeIfPresent(SubjectKind.self, forKey: .kind) ?? .unknown,
            rect: try container.decodeIfPresent(PictureRect.self, forKey: .rect)
                ?? PictureRect(x: 0, y: 0, width: 0, height: 0),
            focus: try container.decodeIfPresent(SubjectFocus.self, forKey: .focus) ?? .unfocused
        )
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(kind, forKey: .kind)
        try container.encode(rect, forKey: .rect)
        try container.encode(focus, forKey: .focus)
    }
}

/// What Cinematic mode sees and where it focuses, sent while it is on.
struct CinematicState: Equatable {
    /// Everything the camera could focus on. At most one is in focus.
    var subjects: [CinematicSubject] = []
    /// There is not enough light for Cinematic mode to work well.
    var lowLight = false
}

extension CinematicState: Codable {
    private enum CodingKeys: String, CodingKey {
        case subjects
        case lowLight
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            subjects: try container.decodeIfPresent([CinematicSubject].self, forKey: .subjects) ?? [],
            lowLight: try container.decodeIfPresent(Bool.self, forKey: .lowLight) ?? false
        )
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(subjects, forKey: .subjects)
        try container.encode(lowLight, forKey: .lowLight)
    }
}

enum ThermalLevel: String, Codable, Equatable {
    case nominal
    case fair
    case serious
    case critical
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

struct Stats: Codable, Equatable {
    var fps: Double = 0
    /// Bits per second actually produced by the encoder.
    var bitrate: Int = 0
    /// Frames dropped since the stream started because the connection fell behind.
    var droppedFrames: Int = 0
    var thermal: ThermalLevel = .nominal
    /// Battery level from 0 to 1.
    var battery: Double = 0
    var charging = false

    private enum CodingKeys: String, CodingKey {
        case fps
        case bitrate
        case droppedFrames
        case thermal
        case battery
        case charging
    }
}

extension Stats {
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            fps: try container.decodeIfPresent(Double.self, forKey: .fps) ?? 0,
            bitrate: try container.decodeIfPresent(Int.self, forKey: .bitrate) ?? 0,
            droppedFrames: try container.decodeIfPresent(Int.self, forKey: .droppedFrames) ?? 0,
            thermal: try container.decodeIfPresent(ThermalLevel.self, forKey: .thermal) ?? .nominal,
            battery: try container.decodeIfPresent(Double.self, forKey: .battery) ?? 0,
            charging: try container.decodeIfPresent(Bool.self, forKey: .charging) ?? false
        )
    }
}

enum LogLevel: String, Codable, Equatable {
    case debug
    case info
    case warning
    case error
    case unknown

    init(from decoder: Decoder) throws {
        self = try decodeTolerant(decoder, fallback: .unknown)
    }
}

struct LogEntry: Codable, Equatable {
    var level: LogLevel = .info
    var tag: String = ""
    var message: String = ""

    private enum CodingKeys: String, CodingKey {
        case level
        case tag
        case message
    }
}

extension LogEntry {
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            level: try container.decodeIfPresent(LogLevel.self, forKey: .level) ?? .info,
            tag: try container.decodeIfPresent(String.self, forKey: .tag) ?? "",
            message: try container.decodeIfPresent(String.self, forKey: .message) ?? ""
        )
    }
}

struct Bye: Codable, Equatable {
    var reason: String = ""

    private enum CodingKeys: String, CodingKey {
        case reason
    }
}

extension Bye {
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(reason: try container.decodeIfPresent(String.self, forKey: .reason) ?? "")
    }
}
