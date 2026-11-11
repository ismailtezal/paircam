// Encrypted records. See "Encrypted records" in `docs/PROTOCOL.md`; this mirrors
// `crates/protocol/src/channel.rs`.

import CryptoKit
import Foundation

private let tagLength = 16

/// The nonce is 4 zero bytes followed by the big-endian record counter.
private func recordNonce(_ counter: UInt64) throws -> ChaChaPoly.Nonce {
    var bytes = Data(count: 4)
    bytes.appendBigEndian(counter)
    return try ChaChaPoly.Nonce(data: bytes)
}

/// Encrypts the records one side sends.
struct Sealer {
    private let key: SymmetricKey
    private var counter: UInt64 = 0

    init(key: Data) {
        self.key = SymmetricKey(data: key)
    }

    /// Encrypts a message into a record body: the ciphertext followed by the 16-byte tag.
    mutating func seal(_ message: Data) throws -> Data {
        let current = counter
        let (next, overflowed) = current.addingReportingOverflow(1)
        guard !overflowed else {
            throw ProtocolError.counterExhausted
        }
        counter = next
        do {
            let box = try ChaChaPoly.seal(message, using: key, nonce: try recordNonce(current))
            var record = Data(capacity: message.count + tagLength)
            record.append(box.ciphertext)
            record.append(box.tag)
            return record
        } catch {
            throw ProtocolError.recordTooLarge(message.count)
        }
    }
}

/// Decrypts the records one side receives.
struct Opener {
    private let key: SymmetricKey
    private var counter: UInt64 = 0

    init(key: Data) {
        self.key = SymmetricKey(data: key)
    }

    /// Decrypts a record body, returning the message.
    mutating func open(_ record: Data) throws -> Data {
        let current = counter
        let (next, overflowed) = current.addingReportingOverflow(1)
        guard !overflowed else {
            throw ProtocolError.counterExhausted
        }
        counter = next
        guard record.count >= tagLength else {
            throw ProtocolError.decrypt
        }
        do {
            let bytes = Data(record)
            let split = bytes.count - tagLength
            let box = try ChaChaPoly.SealedBox(
                nonce: try recordNonce(current),
                ciphertext: bytes.prefix(split),
                tag: bytes.suffix(tagLength)
            )
            return try ChaChaPoly.open(box, using: key)
        } catch {
            throw ProtocolError.decrypt
        }
    }
}
