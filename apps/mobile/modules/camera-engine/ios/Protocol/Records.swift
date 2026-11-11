import Foundation

/// Record framing: a `u32` length followed by the body. See "Records" in `docs/PROTOCOL.md`.
enum RecordFraming {
    static let headerLength = 4

    /// Prefixes a record body with its length.
    static func frame(_ body: Data) throws -> Data {
        guard body.count <= ProtocolConstants.maxRecordLength else {
            throw ProtocolError.recordTooLarge(body.count)
        }
        var framed = Data(capacity: headerLength + body.count)
        framed.appendBigEndian(UInt32(body.count))
        framed.append(body)
        return framed
    }
}

/// Splits a byte stream back into record bodies. Feed it whatever the socket delivers.
struct RecordParser {
    private var buffer = Data()

    mutating func append(_ data: Data) {
        buffer.append(data)
    }

    /// Returns the next complete record body, or `nil` until more bytes arrive.
    mutating func next() throws -> Data? {
        guard buffer.count >= RecordFraming.headerLength else {
            return nil
        }
        let start = buffer.startIndex
        var length = 0
        for index in 0..<RecordFraming.headerLength {
            length = length << 8 | Int(buffer[start + index])
        }
        guard length <= ProtocolConstants.maxRecordLength else {
            throw ProtocolError.recordTooLarge(length)
        }
        let bodyStart = start + RecordFraming.headerLength
        let bodyEnd = bodyStart + length
        guard buffer.endIndex >= bodyEnd else {
            return nil
        }
        let body = Data(buffer[bodyStart..<bodyEnd])
        buffer = Data(buffer[bodyEnd...])
        return body
    }
}
