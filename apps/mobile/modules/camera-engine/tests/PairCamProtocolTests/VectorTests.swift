// Checks the Swift protocol implementation against `protocol/vectors/v0.json`, the same file the
// Rust implementation is tested against. `Resources/v0.json` is a copy; refresh it whenever the
// vectors change.

import CryptoKit
import XCTest

@testable import PairCamProtocol

private struct Vectors: Decodable {
    let version: Int
    let handshakes: [HandshakeVector]
    let messages: [MessageVector]
    let sealed: SealedVector
}

private struct HandshakeVector: Decodable {
    let mode: String
    let hostId: String
    let cameraId: String
    let hostPrivateKey: String
    let cameraPrivateKey: String
    let pairingKeyIn: String
    let record1: String
    let record2: String
    let record3: String
    let hostToCameraKey: String
    let cameraToHostKey: String
    let pairingKeyOut: String
    let pairingCode: String
}

private struct MessageVector: Decodable {
    let name: String
    let exact: Bool
    let bytes: String
}

private struct SealedVector: Decodable {
    struct SealedRecord: Decodable {
        let counter: Int
        let plaintext: String
        let record: String
    }

    let key: String
    let records: [SealedRecord]
}

private func bytes(_ hex: String, file: StaticString = #filePath, line: UInt = #line) -> Data {
    guard let data = Data(hex: hex) else {
        XCTFail("invalid hex in the vector file", file: file, line: line)
        return Data()
    }
    return data
}

private func loadVectors() throws -> Vectors {
    let url = try XCTUnwrap(Bundle.module.url(forResource: "v0", withExtension: "json"))
    return try JSONDecoder().decode(Vectors.self, from: Data(contentsOf: url))
}

final class VectorTests: XCTestCase {
    func testVectorFileMatchesTheImplementedVersion() throws {
        let vectors = try loadVectors()
        XCTAssertEqual(vectors.version, Int(ProtocolConstants.version))
        XCTAssertEqual(vectors.handshakes.count, 2)
    }

    func testCameraHandshakeReproducesTheVectors() throws {
        for vector in try loadVectors().handshakes {
            let host = try HostHello(record: bytes(vector.record1))
            XCTAssertEqual(host.mode, vector.mode == "pair" ? .pair : .resume)
            XCTAssertEqual(host.hostId.hex, vector.hostId)
            XCTAssertEqual(host.version, ProtocolConstants.version)

            let cameraId = try XCTUnwrap(DeviceId(hex: vector.cameraId))
            let privateKey = try Curve25519.KeyAgreement.PrivateKey(
                rawRepresentation: bytes(vector.cameraPrivateKey)
            )
            let handshake = CameraHandshake(host: host, cameraId: cameraId, privateKey: privateKey)
            XCTAssertEqual(handshake.hello.hexString, vector.record2)

            let keys = try handshake.finish(
                hostKey: bytes(vector.record3),
                pairingKey: bytes(vector.pairingKeyIn)
            )
            XCTAssertEqual(keys.hostToCamera.hexString, vector.hostToCameraKey)
            XCTAssertEqual(keys.cameraToHost.hexString, vector.cameraToHostKey)
            XCTAssertEqual(keys.pairingKey.hexString, vector.pairingKeyOut)
            XCTAssertEqual(keys.pairingCodeDigits, vector.pairingCode)
        }
    }

    func testHostSideOfTheVectorsDerivesTheSameKeys() throws {
        for vector in try loadVectors().handshakes {
            let hostKey = try Curve25519.KeyAgreement.PrivateKey(
                rawRepresentation: bytes(vector.hostPrivateKey)
            )
            let hostPublicKey = hostKey.publicKey.rawRepresentation
            XCTAssertEqual(hostPublicKey.hexString, vector.record3)

            let record1 = bytes(vector.record1)
            XCTAssertEqual(
                Data(SHA256.hash(data: hostPublicKey)).hexString,
                Data(record1.suffix(32)).hexString
            )

            let record2 = bytes(vector.record2)
            let keys = try CameraHandshake.deriveKeys(
                privateKey: hostKey,
                peerPublicKey: Data(record2.suffix(32)),
                pairingKey: bytes(vector.pairingKeyIn),
                records: [record1, record2, bytes(vector.record3)]
            )
            XCTAssertEqual(keys.hostToCamera.hexString, vector.hostToCameraKey)
            XCTAssertEqual(keys.cameraToHost.hexString, vector.cameraToHostKey)
            XCTAssertEqual(keys.pairingKey.hexString, vector.pairingKeyOut)
            XCTAssertEqual(keys.pairingCodeDigits, vector.pairingCode)
        }
    }

    func testSealedRecordsMatchByteForByte() throws {
        let sealed = try loadVectors().sealed
        var sealer = Sealer(key: bytes(sealed.key))
        var opener = Opener(key: bytes(sealed.key))

        for (index, record) in sealed.records.enumerated() {
            XCTAssertEqual(record.counter, index)
            let plaintext = bytes(record.plaintext)
            let body = try sealer.seal(plaintext)
            XCTAssertEqual(body.hexString, record.record)
            XCTAssertEqual(try opener.open(bytes(record.record)), plaintext)
        }
    }

    func testBinaryMessagesMatchByteForByte() throws {
        let expected: [String: Message] = [
            "videoConfig": .videoConfig(
                VideoConfig(
                    codec: .hevc,
                    width: 1920,
                    height: 1080,
                    rotation: 90,
                    mirrored: true,
                    record: Data([0x01, 0x02, 0x03, 0x04, 0x05])
                )
            ),
            "videoFrame": .videoFrame(
                VideoFrame(
                    timestampMicros: 0x0001_0203_0405_0607,
                    keyframe: true,
                    data: Data([0x00, 0x00, 0x00, 0x02, 0x26, 0x01])
                )
            ),
            "startStream": .startStream,
            "stopStream": .stopStream,
            "requestKeyframe": .requestKeyframe,
            "ping": .ping(1_000_000),
            "pong": .pong(1_000_000),
        ]

        let exact = try loadVectors().messages.filter { $0.exact }
        XCTAssertEqual(Set(exact.map { $0.name }), Set(expected.keys))
        for vector in exact {
            let message = try XCTUnwrap(expected[vector.name])
            XCTAssertEqual(try message.encode().hexString, vector.bytes, vector.name)
            XCTAssertEqual(try Message.decode(bytes(vector.bytes)), message, vector.name)
        }
    }

    func testJsonMessagesDecodeToTheSameValues() throws {
        let expected: [String: Message] = [
            "hello": .hello(
                Hello(name: "Test iPhone", appVersion: "0.1.0", platform: "ios", model: "iPhone16,2")
            ),
            "control": .control(ControlValues(zoom: 2.0, torch: true)),
        ]

        let loose = try loadVectors().messages.filter { !$0.exact }
        XCTAssertEqual(Set(loose.map { $0.name }), Set(expected.keys))
        for vector in loose {
            let message = try XCTUnwrap(expected[vector.name])
            XCTAssertEqual(try Message.decode(bytes(vector.bytes)), message, vector.name)
            XCTAssertEqual(try Message.decode(try message.encode()), message, vector.name)
        }
    }
}
