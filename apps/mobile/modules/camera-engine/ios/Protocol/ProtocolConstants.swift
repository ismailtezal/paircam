// The PairCam phone-to-desktop protocol, camera side. The specification is `docs/PROTOCOL.md`
// and the reference implementation is `crates/protocol`.
//
// Everything in this folder imports only Foundation and CryptoKit so that it can be built and
// tested on its own with Swift Package Manager (see `Package.swift` in the module root).

import Foundation

enum ProtocolConstants {
    /// The protocol version this implementation speaks.
    static let version: UInt8 = 0

    /// The TCP port the camera listens on unless it had to pick another one.
    static let defaultPort: UInt16 = 47810

    /// The Bonjour service type the camera advertises.
    static let serviceType = "_paircam._tcp"

    /// The largest record body either side accepts.
    static let maxRecordLength = 8 * 1024 * 1024
}

enum ProtocolError: Error, Equatable {
    case recordTooLarge(Int)
    case badMagic
    case unsupportedVersion(UInt8)
    case malformed(String)
    case commitmentMismatch
    case invalidKeyExchange
    case decrypt
    case counterExhausted
    case json(String)
}
