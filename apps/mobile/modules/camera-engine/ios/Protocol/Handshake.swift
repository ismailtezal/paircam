// The three-record handshake, camera side, and the keys it derives.
// See "Handshake" in `docs/PROTOCOL.md`; this mirrors `crates/protocol/src/handshake.rs`.

import CryptoKit
import Foundation

private let magic = Data("PCAM".utf8)
private let hostHelloLength = 4 + 1 + 1 + 16 + 32
private let hostKeyLength = 32

/// A device's permanent random identifier.
struct DeviceId: Hashable {
    static let byteCount = 16

    let bytes: Data

    init?(bytes: Data) {
        guard bytes.count == DeviceId.byteCount else {
            return nil
        }
        self.bytes = Data(bytes)
    }

    /// Parses the 32 hex characters used in discovery records.
    init?(hex: String) {
        guard hex.utf8.count == DeviceId.byteCount * 2, let bytes = Data(hex: hex) else {
            return nil
        }
        self.bytes = bytes
    }

    static func random() -> DeviceId {
        var generator = SystemRandomNumberGenerator()
        let bytes = (0..<byteCount).map { _ in UInt8.random(in: UInt8.min...UInt8.max, using: &generator) }
        return DeviceId(bytes: Data(bytes))!
    }

    var hex: String {
        bytes.hexString
    }
}

/// Whether the host wants to pair for the first time or reconnect to a paired camera.
enum HandshakeMode: UInt8 {
    case pair = 0
    case resume = 1
}

/// The camera's answer to a connection attempt.
enum HandshakeStatus: UInt8 {
    case ok = 0
    case unsupportedVersion = 1
    case notPaired = 2
    case busy = 3
}

/// Record 1, as the camera reads it, together with its exact bytes.
struct HostHello: Equatable {
    let version: UInt8
    let mode: HandshakeMode
    let hostId: DeviceId
    let commitment: Data
    /// The record body exactly as received. The key derivation hashes it.
    let body: Data

    /// Parses record 1. An unsupported version is reported before the length is checked so the
    /// camera can answer a newer host with a refusal it understands.
    init(record body: Data) throws {
        let bytes = [UInt8](body)
        guard bytes.count >= 4, Data(bytes[0..<4]) == magic else {
            throw ProtocolError.badMagic
        }
        guard bytes.count >= 6 else {
            throw ProtocolError.malformed("host hello")
        }
        let version = bytes[4]
        guard version == ProtocolConstants.version else {
            throw ProtocolError.unsupportedVersion(version)
        }
        guard bytes.count == hostHelloLength else {
            throw ProtocolError.malformed("host hello")
        }
        guard let mode = HandshakeMode(rawValue: bytes[5]) else {
            throw ProtocolError.malformed("host hello mode")
        }
        guard let hostId = DeviceId(bytes: Data(bytes[6..<22])) else {
            throw ProtocolError.malformed("host hello")
        }
        self.version = version
        self.mode = mode
        self.hostId = hostId
        self.commitment = Data(bytes[22..<54])
        self.body = Data(bytes)
    }
}

/// Everything both sides derive from a finished handshake.
struct SessionKeys: Equatable {
    /// Decrypts records the host sends.
    let hostToCamera: Data
    /// Encrypts records the camera sends.
    let cameraToHost: Data
    /// Stored by both sides after a successful pairing, then used to resume.
    let pairingKey: Data
    /// The number both apps show while pairing, below 1 000 000.
    let pairingCode: UInt32

    /// The pairing code as the six digits shown to the user.
    var pairingCodeDigits: String {
        let digits = String(pairingCode)
        return String(repeating: "0", count: max(0, 6 - digits.count)) + digits
    }
}

/// The camera side of the handshake.
///
/// ```text
/// let host = try HostHello(record: receive())
/// let handshake = CameraHandshake(host: host, cameraId: cameraId)
/// send(handshake.hello)
/// let keys = try handshake.finish(hostKey: receive(), pairingKey: pairingKey(for: host.hostId))
/// ```
struct CameraHandshake {
    /// The value used in place of a pairing key while pairing for the first time.
    static let unpaired = Data(count: 32)

    /// The body of record 2.
    let hello: Data

    private let privateKey: Curve25519.KeyAgreement.PrivateKey
    private let hostHello: Data
    private let commitment: Data

    /// Accepts the host. Each connection must use a fresh private key, which is the default.
    init(
        host: HostHello,
        cameraId: DeviceId,
        privateKey: Curve25519.KeyAgreement.PrivateKey = Curve25519.KeyAgreement.PrivateKey()
    ) {
        self.privateKey = privateKey
        self.hostHello = host.body
        self.commitment = host.commitment
        self.hello = CameraHandshake.helloBody(
            status: .ok,
            cameraId: cameraId,
            publicKey: privateKey.publicKey.rawRepresentation
        )
    }

    /// The body of a record 2 that turns the host away.
    static func refusal(status: HandshakeStatus, cameraId: DeviceId) -> Data {
        helloBody(status: status, cameraId: cameraId, publicKey: Data(count: 32))
    }

    /// Finishes the handshake with record 3 from the host.
    ///
    /// `pairingKey` is `CameraHandshake.unpaired` in pair mode and the stored key in resume mode.
    func finish(hostKey: Data, pairingKey: Data) throws -> SessionKeys {
        guard hostKey.count == hostKeyLength else {
            throw ProtocolError.malformed("host key")
        }
        let hostPublicKey = Data(hostKey)
        guard Data(SHA256.hash(data: hostPublicKey)) == commitment else {
            throw ProtocolError.commitmentMismatch
        }
        return try CameraHandshake.deriveKeys(
            privateKey: privateKey,
            peerPublicKey: hostPublicKey,
            pairingKey: pairingKey,
            records: [hostHello, hello, hostPublicKey]
        )
    }

    private static func helloBody(status: HandshakeStatus, cameraId: DeviceId, publicKey: Data) -> Data {
        var body = Data(capacity: 4 + 1 + 1 + 16 + 32)
        body.append(magic)
        body.append(ProtocolConstants.version)
        body.append(status.rawValue)
        body.append(cameraId.bytes)
        body.append(publicKey)
        return body
    }

    /// The key schedule from "Keys" in the specification. Shared by both roles.
    static func deriveKeys(
        privateKey: Curve25519.KeyAgreement.PrivateKey,
        peerPublicKey: Data,
        pairingKey: Data,
        records: [Data]
    ) throws -> SessionKeys {
        let sharedBytes: Data
        do {
            let peer = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerPublicKey)
            let shared = try privateKey.sharedSecretFromKeyAgreement(with: peer)
            sharedBytes = shared.withUnsafeBytes { Data($0) }
        } catch {
            throw ProtocolError.invalidKeyExchange
        }
        // An all-zero result means the peer sent a low-order point and contributed nothing.
        guard sharedBytes.contains(where: { $0 != 0 }) else {
            throw ProtocolError.invalidKeyExchange
        }

        var transcriptHash = SHA256()
        for record in records {
            transcriptHash.update(data: record)
        }
        let transcript = Data(transcriptHash.finalize())

        let pseudoRandomKey = HKDF<SHA256>.extract(
            inputKeyMaterial: SymmetricKey(data: sharedBytes),
            salt: pairingKey
        )
        func expand(_ label: String, byteCount: Int) -> Data {
            var info = Data(label.utf8)
            info.append(transcript)
            let key = HKDF<SHA256>.expand(
                pseudoRandomKey: pseudoRandomKey,
                info: info,
                outputByteCount: byteCount
            )
            return key.withUnsafeBytes { Data($0) }
        }

        let code = expand("paircam v0 sas", byteCount: 4)
        var codeValue: UInt32 = 0
        for byte in code {
            codeValue = codeValue << 8 | UInt32(byte)
        }

        return SessionKeys(
            hostToCamera: expand("paircam v0 h2c", byteCount: 32),
            cameraToHost: expand("paircam v0 c2h", byteCount: 32),
            pairingKey: expand("paircam v0 pairing", byteCount: 32),
            pairingCode: codeValue % 1_000_000
        )
    }
}
