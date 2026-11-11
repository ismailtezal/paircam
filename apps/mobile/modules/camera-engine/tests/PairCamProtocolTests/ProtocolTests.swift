// Behaviour the vectors do not cover: refusals, malformed input, tolerance of newer peers.
// These mirror the unit tests in `crates/protocol/src`.

import CryptoKit
import XCTest

@testable import PairCamProtocol

private let hostId = DeviceId(bytes: Data(repeating: 0x11, count: 16))!
private let cameraId = DeviceId(bytes: Data(repeating: 0x22, count: 16))!

/// Builds record 1 and record 3 the way the host does.
private func hostRecords(
    mode: HandshakeMode,
    key: Curve25519.KeyAgreement.PrivateKey
) -> (record1: Data, record3: Data) {
    let publicKey = key.publicKey.rawRepresentation
    var record1 = Data("PCAM".utf8)
    record1.append(ProtocolConstants.version)
    record1.append(mode.rawValue)
    record1.append(hostId.bytes)
    record1.append(Data(SHA256.hash(data: publicKey)))
    return (record1, publicKey)
}

final class HandshakeTests: XCTestCase {
    func testBothSidesDeriveTheSameKeysWhenPairing() throws {
        let hostKey = Curve25519.KeyAgreement.PrivateKey()
        let (record1, record3) = hostRecords(mode: .pair, key: hostKey)

        let host = try HostHello(record: record1)
        XCTAssertEqual(host.mode, .pair)
        XCTAssertEqual(host.hostId, hostId)

        let camera = CameraHandshake(host: host, cameraId: cameraId)
        let cameraKeys = try camera.finish(hostKey: record3, pairingKey: CameraHandshake.unpaired)
        let hostKeys = try CameraHandshake.deriveKeys(
            privateKey: hostKey,
            peerPublicKey: Data(camera.hello.suffix(32)),
            pairingKey: CameraHandshake.unpaired,
            records: [record1, camera.hello, record3]
        )

        XCTAssertEqual(cameraKeys, hostKeys)
        XCTAssertNotEqual(cameraKeys.hostToCamera, cameraKeys.cameraToHost)
        XCTAssertLessThan(cameraKeys.pairingCode, 1_000_000)
        XCTAssertEqual(cameraKeys.pairingCodeDigits.count, 6)
    }

    func testResumingWithTheWrongKeyGivesDifferentKeys() throws {
        let hostKey = Curve25519.KeyAgreement.PrivateKey()
        let (record1, record3) = hostRecords(mode: .resume, key: hostKey)
        let camera = CameraHandshake(host: try HostHello(record: record1), cameraId: cameraId)

        let cameraKeys = try camera.finish(
            hostKey: record3,
            pairingKey: Data(repeating: 2, count: 32)
        )
        let hostKeys = try CameraHandshake.deriveKeys(
            privateKey: hostKey,
            peerPublicKey: Data(camera.hello.suffix(32)),
            pairingKey: Data(repeating: 1, count: 32),
            records: [record1, camera.hello, record3]
        )
        XCTAssertNotEqual(cameraKeys.hostToCamera, hostKeys.hostToCamera)
        XCTAssertNotEqual(cameraKeys.cameraToHost, hostKeys.cameraToHost)
    }

    func testCameraRejectsAHostKeyThatBreaksTheCommitment() throws {
        let (record1, _) = hostRecords(mode: .pair, key: Curve25519.KeyAgreement.PrivateKey())
        let camera = CameraHandshake(host: try HostHello(record: record1), cameraId: cameraId)
        let otherKey = Curve25519.KeyAgreement.PrivateKey().publicKey.rawRepresentation

        XCTAssertThrowsError(
            try camera.finish(hostKey: otherKey, pairingKey: CameraHandshake.unpaired)
        ) { error in
            XCTAssertEqual(error as? ProtocolError, .commitmentMismatch)
        }
        XCTAssertThrowsError(
            try camera.finish(hostKey: Data(count: 31), pairingKey: CameraHandshake.unpaired)
        ) { error in
            XCTAssertEqual(error as? ProtocolError, .malformed("host key"))
        }
    }

    func testRefusalCarriesTheStatusAndAZeroKey() {
        let refusal = [UInt8](CameraHandshake.refusal(status: .notPaired, cameraId: cameraId))
        XCTAssertEqual(refusal.count, 54)
        XCTAssertEqual(Data(refusal[0..<4]), Data("PCAM".utf8))
        XCTAssertEqual(refusal[4], ProtocolConstants.version)
        XCTAssertEqual(refusal[5], HandshakeStatus.notPaired.rawValue)
        XCTAssertEqual(Data(refusal[6..<22]), cameraId.bytes)
        XCTAssertEqual(Data(refusal[22..<54]), Data(count: 32))
    }

    func testRejectsForeignTruncatedAndFutureRecords() {
        XCTAssertThrowsError(try HostHello(record: Data("GET / HTTP/1.1".utf8))) { error in
            XCTAssertEqual(error as? ProtocolError, .badMagic)
        }
        XCTAssertThrowsError(try HostHello(record: Data("PCAM".utf8) + Data([0]))) { error in
            XCTAssertEqual(error as? ProtocolError, .malformed("host hello"))
        }

        var future = [UInt8](hostRecords(mode: .pair, key: Curve25519.KeyAgreement.PrivateKey()).record1)
        future[4] = 9
        XCTAssertThrowsError(try HostHello(record: Data(future))) { error in
            XCTAssertEqual(error as? ProtocolError, .unsupportedVersion(9))
        }

        var badMode = [UInt8](hostRecords(mode: .pair, key: Curve25519.KeyAgreement.PrivateKey()).record1)
        badMode[5] = 7
        XCTAssertThrowsError(try HostHello(record: Data(badMode))) { error in
            XCTAssertEqual(error as? ProtocolError, .malformed("host hello mode"))
        }
    }

    func testDeviceIdsRoundTripThroughHex() {
        let id = DeviceId(bytes: Data(repeating: 0xAB, count: 16))!
        XCTAssertEqual(DeviceId(hex: id.hex), id)
        XCTAssertNil(DeviceId(hex: "abc"))
        XCTAssertNil(DeviceId(hex: String(repeating: "zz", count: 16)))
        XCTAssertNotEqual(DeviceId.random(), DeviceId.random())
    }
}

final class ChannelTests: XCTestCase {
    func testSealsAndOpensInOrder() throws {
        let key = Data(repeating: 7, count: 32)
        var sealer = Sealer(key: key)
        var opener = Opener(key: key)

        for text in [Data("one".utf8), Data(), Data("three".utf8)] {
            let record = try sealer.seal(text)
            XCTAssertEqual(record.count, text.count + 16)
            XCTAssertEqual(try opener.open(record), text)
        }
    }

    func testRejectsReplayedTamperedAndForeignRecords() throws {
        let key = Data(repeating: 7, count: 32)
        var sealer = Sealer(key: key)
        var opener = Opener(key: key)

        let first = try sealer.seal(Data("first".utf8))
        XCTAssertEqual(try opener.open(first), Data("first".utf8))
        XCTAssertThrowsError(try opener.open(first)) { error in
            XCTAssertEqual(error as? ProtocolError, .decrypt)
        }

        var tamperedBytes = [UInt8](first)
        tamperedBytes[0] ^= 1
        var freshOpener = Opener(key: key)
        XCTAssertThrowsError(try freshOpener.open(Data(tamperedBytes))) { error in
            XCTAssertEqual(error as? ProtocolError, .decrypt)
        }

        var wrongKeyOpener = Opener(key: Data(repeating: 2, count: 32))
        XCTAssertThrowsError(try wrongKeyOpener.open(first)) { error in
            XCTAssertEqual(error as? ProtocolError, .decrypt)
        }

        var shortOpener = Opener(key: key)
        XCTAssertThrowsError(try shortOpener.open(Data(count: 15))) { error in
            XCTAssertEqual(error as? ProtocolError, .decrypt)
        }
    }
}

final class RecordTests: XCTestCase {
    func testRoundTripsRecordsDeliveredInPieces() throws {
        var wire = Data()
        wire.append(try RecordFraming.frame(Data("first".utf8)))
        wire.append(try RecordFraming.frame(Data()))
        wire.append(try RecordFraming.frame(Data("third".utf8)))

        var parser = RecordParser()
        var bodies = [Data]()
        for byte in [UInt8](wire) {
            parser.append(Data([byte]))
            while let body = try parser.next() {
                bodies.append(body)
            }
        }
        XCTAssertEqual(bodies, [Data("first".utf8), Data(), Data("third".utf8)])
        XCTAssertNil(try parser.next())
    }

    func testRejectsOversizedLengths() {
        let tooLarge = ProtocolConstants.maxRecordLength + 1
        var wire = Data()
        wire.appendBigEndian(UInt32(tooLarge))
        var parser = RecordParser()
        parser.append(wire)
        XCTAssertThrowsError(try parser.next()) { error in
            XCTAssertEqual(error as? ProtocolError, .recordTooLarge(tooLarge))
        }
        XCTAssertThrowsError(try RecordFraming.frame(Data(count: tooLarge)))
    }
}

final class MessageTests: XCTestCase {
    private func roundTrip(
        _ message: Message,
        file: StaticString = #filePath,
        line: UInt = #line
    ) throws {
        XCTAssertEqual(try Message.decode(try message.encode()), message, file: file, line: line)
    }

    func testRoundTripsEveryMessage() throws {
        try roundTrip(
            .hello(Hello(name: "Kitchen iPhone", appVersion: "0.1.0", platform: "ios", model: "iPhone16,2"))
        )
        try roundTrip(.pairResult(PairResult(accepted: true)))
        try roundTrip(
            .capabilities(
                Capabilities(
                    cameras: [
                        CameraInfo(
                            id: "back-wide",
                            name: "Back Camera",
                            position: .back,
                            lens: .wide,
                            formats: [VideoFormat(width: 1920, height: 1080, maxFps: 60)]
                        ),
                    ],
                    codecs: [.hevc, .h264],
                    controls: ControlCapabilities(
                        zoom: ValueRange(min: 1, max: 10),
                        focus: [.auto, .locked],
                        torch: true
                    )
                )
            )
        )
        try roundTrip(
            .configure(
                Configure(
                    camera: "back-wide",
                    width: 1920,
                    height: 1080,
                    fps: 30,
                    codec: .hevc,
                    bitrate: 12_000_000,
                    keyframeInterval: 2
                )
            )
        )
        try roundTrip(.startStream)
        try roundTrip(.stopStream)
        try roundTrip(
            .videoConfig(
                VideoConfig(
                    codec: .h264,
                    width: 1280,
                    height: 720,
                    rotation: 0,
                    mirrored: false,
                    record: Data([1, 2, 3, 4])
                )
            )
        )
        try roundTrip(
            .videoFrame(VideoFrame(timestampMicros: 1_234_567, keyframe: false, data: Data([0, 0, 0, 1, 0x65])))
        )
        try roundTrip(.requestKeyframe)
        try roundTrip(.control(ControlValues(zoom: 2)))
        try roundTrip(.controlState(ControlValues(zoom: 1, focus: .auto, exposureBias: 0, torch: false)))
        try roundTrip(
            .controlState(ControlValues(zoom: 2, torch: false, faceTracking: true, gestures: false))
        )
        try roundTrip(.control(ControlValues(faceTracking: true)))
        try roundTrip(.control(ControlValues(gestures: true)))
        try roundTrip(.subject(SubjectUpdate(face: PictureRect(x: 0.41, y: 0.22, width: 0.18, height: 0.24))))
        try roundTrip(.subject(SubjectUpdate(face: nil)))
        try roundTrip(.gesture(GestureEvent(kind: .palm, phase: .recognised)))
        try roundTrip(.gesture(GestureEvent(kind: .frame, phase: .recognised)))
        try roundTrip(.gesture(GestureEvent(kind: .spread, phase: .began)))
        try roundTrip(.gesture(GestureEvent(kind: .spread, phase: .ended)))
        try roundTrip(
            .stats(
                Stats(
                    fps: 29.97,
                    bitrate: 11_840_000,
                    droppedFrames: 2,
                    thermal: .fair,
                    battery: 0.82,
                    charging: true
                )
            )
        )
        try roundTrip(.log(LogEntry(level: .warning, tag: "encoder", message: "dropped a frame")))
        try roundTrip(.ping(42))
        try roundTrip(.pong(42))
        try roundTrip(.bye(Bye(reason: "closed by user")))
    }

    func testControlOnlyCarriesTheChangedFields() throws {
        let message = try Message.control(ControlValues(torch: true)).encode()
        let payload = String(decoding: [UInt8](message).dropFirst(), as: UTF8.self)
        XCTAssertEqual(payload, #"{"torch":true}"#)
    }

    func testUnsupportedControlsAreAbsentFromCapabilities() throws {
        let message = try Message.capabilities(Capabilities()).encode()
        let payload = Data([UInt8](message).dropFirst())
        let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any]
        XCTAssertEqual((object?["controls"] as? [String: Any])?.isEmpty, true)
    }

    func testCapabilitiesAnnounceFaceTrackingAndGestures() throws {
        var controls = ControlCapabilities()
        controls.faceTracking = true
        controls.gestures = [.palm, .frame, .spread]
        let message = try Message.capabilities(Capabilities(controls: controls)).encode()
        let payload = Data([UInt8](message).dropFirst())
        let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any]
        let encoded = try XCTUnwrap(object?["controls"] as? [String: Any])
        XCTAssertEqual(encoded["faceTracking"] as? Bool, true)
        XCTAssertEqual(encoded["gestures"] as? [String], ["palm", "frame", "spread"])

        var spec = Data([MessageKind.capabilities])
        spec.append(Data(#"{"controls":{"faceTracking":true,"gestures":["palm","frame","spread","wave"]}}"#.utf8))
        guard case .capabilities(let decoded) = try Message.decode(spec) else {
            return XCTFail("expected capabilities")
        }
        XCTAssertTrue(decoded.controls.faceTracking)
        XCTAssertEqual(decoded.controls.gestures, [.palm, .frame, .spread, .unknown])
    }

    func testControlCarriesFaceTrackingAndGesturesAsBooleans() throws {
        let message = try Message.control(ControlValues(faceTracking: true)).encode()
        let payload = String(decoding: [UInt8](message).dropFirst(), as: UTF8.self)
        XCTAssertEqual(payload, #"{"faceTracking":true}"#)

        var spec = Data([MessageKind.control])
        spec.append(Data(#"{"gestures":true}"#.utf8))
        XCTAssertEqual(try Message.decode(spec), .control(ControlValues(gestures: true)))

        var older = Data([MessageKind.controlState])
        older.append(Data(#"{"zoom":1.0}"#.utf8))
        guard case .controlState(let state) = try Message.decode(older) else {
            return XCTFail("expected controlState")
        }
        XCTAssertNil(state.faceTracking)
        XCTAssertNil(state.gestures)
    }

    func testSubjectWritesAnExplicitNullWhenNobodyIsInView() throws {
        let message = try Message.subject(SubjectUpdate(face: nil)).encode()
        XCTAssertEqual(message.first, 0x22)
        let payload = String(decoding: [UInt8](message).dropFirst(), as: UTF8.self)
        XCTAssertEqual(payload, #"{"face":null}"#)

        var spec = Data([MessageKind.subject])
        spec.append(Data(#"{"face":{"x":0.41,"y":0.22,"width":0.18,"height":0.24}}"#.utf8))
        XCTAssertEqual(
            try Message.decode(spec),
            .subject(SubjectUpdate(face: PictureRect(x: 0.41, y: 0.22, width: 0.18, height: 0.24)))
        )

        var missing = Data([MessageKind.subject])
        missing.append(Data("{}".utf8))
        XCTAssertEqual(try Message.decode(missing), .subject(SubjectUpdate(face: nil)))
    }

    func testGestureMatchesTheSpecification() throws {
        let message = try Message.gesture(GestureEvent(kind: .palm, phase: .recognised)).encode()
        XCTAssertEqual(message.first, 0x23)
        let payload = Data([UInt8](message).dropFirst())
        let object = try JSONSerialization.jsonObject(with: payload) as? [String: String]
        XCTAssertEqual(object, ["kind": "palm", "phase": "recognised"])

        var future = Data([MessageKind.gesture])
        future.append(Data(#"{"kind":"wave","phase":"held","extra":1}"#.utf8))
        XCTAssertEqual(try Message.decode(future), .gesture(GestureEvent(kind: .unknown, phase: .unknown)))
    }

    func testSkipsUnknownMessageTypes() throws {
        XCTAssertEqual(try Message.decode(Data([0x6A, 1, 2, 3])), .unknown(0x6A))
    }

    func testToleratesUnknownFieldsAndValues() throws {
        var message = Data([MessageKind.stats])
        message.append(Data(#"{"fps":30,"thermal":"melting","newField":[1,2]}"#.utf8))
        guard case .stats(let stats) = try Message.decode(message) else {
            return XCTFail("expected stats")
        }
        XCTAssertEqual(stats.fps, 30)
        XCTAssertEqual(stats.thermal, .unknown)
        XCTAssertEqual(stats.droppedFrames, 0)
    }

    func testRejectsTruncatedBinaryMessages() throws {
        XCTAssertThrowsError(try Message.decode(Data()))
        XCTAssertThrowsError(try Message.decode(Data([MessageKind.videoFrame, 0, 0])))
        XCTAssertThrowsError(try Message.decode(Data([MessageKind.ping, 0])))

        let config = try Message.videoConfig(
            VideoConfig(
                codec: .h264,
                width: 1280,
                height: 720,
                rotation: 0,
                mirrored: false,
                record: Data(repeating: 9, count: 8)
            )
        ).encode()
        XCTAssertThrowsError(try Message.decode(Data([UInt8](config).dropLast())))
    }
}
