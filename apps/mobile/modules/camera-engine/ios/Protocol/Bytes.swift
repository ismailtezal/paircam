import Foundation

extension Data {
    /// Appends an integer in network byte order.
    mutating func appendBigEndian<T: FixedWidthInteger>(_ value: T) {
        Swift.withUnsafeBytes(of: value.bigEndian) { bytes in
            append(contentsOf: bytes)
        }
    }

    /// Parses lowercase or uppercase hex. Returns `nil` for odd lengths and non-hex characters.
    init?(hex: String) {
        let characters = Array(hex.utf8)
        guard characters.count % 2 == 0 else {
            return nil
        }
        var bytes = [UInt8]()
        bytes.reserveCapacity(characters.count / 2)
        var index = 0
        while index < characters.count {
            guard
                let high = Data.hexValue(characters[index]),
                let low = Data.hexValue(characters[index + 1])
            else {
                return nil
            }
            bytes.append(high << 4 | low)
            index += 2
        }
        self.init(bytes)
    }

    /// Lowercase hex, two characters per byte.
    var hexString: String {
        let digits = Array("0123456789abcdef".utf8)
        var characters = [UInt8]()
        characters.reserveCapacity(count * 2)
        for byte in self {
            characters.append(digits[Int(byte >> 4)])
            characters.append(digits[Int(byte & 0x0F)])
        }
        return String(decoding: characters, as: UTF8.self)
    }

    private static func hexValue(_ character: UInt8) -> UInt8? {
        switch character {
        case UInt8(ascii: "0")...UInt8(ascii: "9"):
            return character - UInt8(ascii: "0")
        case UInt8(ascii: "a")...UInt8(ascii: "f"):
            return character - UInt8(ascii: "a") + 10
        case UInt8(ascii: "A")...UInt8(ascii: "F"):
            return character - UInt8(ascii: "A") + 10
        default:
            return nil
        }
    }
}

/// Reads big-endian integers and byte runs from a buffer, front to back.
struct ByteReader {
    private let bytes: [UInt8]
    private var offset = 0

    init(_ data: Data) {
        bytes = [UInt8](data)
    }

    var remaining: Int {
        bytes.count - offset
    }

    mutating func readUInt8() -> UInt8? {
        guard remaining >= 1 else {
            return nil
        }
        defer { offset += 1 }
        return bytes[offset]
    }

    mutating func readUInt16() -> UInt16? {
        readInteger(byteCount: 2).map { UInt16($0) }
    }

    mutating func readUInt32() -> UInt32? {
        readInteger(byteCount: 4).map { UInt32($0) }
    }

    mutating func readUInt64() -> UInt64? {
        readInteger(byteCount: 8)
    }

    mutating func readBytes(_ count: Int) -> Data? {
        guard count >= 0, remaining >= count else {
            return nil
        }
        defer { offset += count }
        return Data(bytes[offset..<(offset + count)])
    }

    mutating func readRemaining() -> Data {
        defer { offset = bytes.count }
        return Data(bytes[offset...])
    }

    private mutating func readInteger(byteCount: Int) -> UInt64? {
        guard remaining >= byteCount else {
            return nil
        }
        var value: UInt64 = 0
        for index in 0..<byteCount {
            value = value << 8 | UInt64(bytes[offset + index])
        }
        offset += byteCount
        return value
    }
}
