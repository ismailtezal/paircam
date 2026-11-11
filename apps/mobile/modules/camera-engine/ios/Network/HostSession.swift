import Foundation
import Network

/// Seconds since boot on a clock that never jumps.
func monotonicNow() -> TimeInterval {
    ProcessInfo.processInfo.systemUptime
}

protocol HostSessionDelegate: AnyObject {
    /// Record 1 arrived. Decide whether this host may continue, and with which pairing key.
    func hostSession(_ session: HostSession, admit hello: HostHello) -> HostSession.Admission

    /// The host's `hello` decrypted, which proves it derived the same keys.
    func hostSessionDidAuthenticate(_ session: HostSession)

    /// A message arrived on an established session.
    func hostSession(_ session: HostSession, didReceive message: Message)

    func hostSession(_ session: HostSession, didCloseWith reason: HostSession.CloseReason)
}

/// One TCP connection from a desktop app: the handshake, then encrypted messages both ways.
///
/// A session is confined to the queue given to `init`; every method must be called on it and every
/// delegate call arrives on it.
final class HostSession {
    enum Admission {
        case accept(pairingKey: Data)
        case refuse(HandshakeStatus)
    }

    enum CloseReason {
        /// The camera turned the host away during the handshake.
        case refused(HandshakeStatus)
        /// The host said goodbye.
        case peerBye(String)
        /// The camera said goodbye.
        case localBye(String)
        /// The host closed the connection without a goodbye.
        case peerClosed
        /// The host stopped answering or stopped reading.
        case timeout
        case failed(String)
    }

    private enum Phase {
        case awaitingHostHello
        case awaitingHostKey(CameraHandshake, pairingKey: Data)
        case awaitingPeerHello
        /// Keys are proven. In pair mode the user has not decided yet.
        case authenticated
        case established
        case closed
    }

    private static let handshakeTimeout: TimeInterval = 10
    private static let pingInterval: TimeInterval = 2
    private static let pongTimeout: TimeInterval = 5
    private static let writeStallTimeout: TimeInterval = 5
    private static let receiveChunk = 64 * 1024

    weak var delegate: HostSessionDelegate?

    private(set) var hostHello: HostHello?
    private(set) var peerHello: Hello?
    private(set) var keys: SessionKeys?

    /// Above this many queued bytes, video frames are dropped until the next keyframe.
    var videoQueueLimit = 512 * 1024

    private let connection: NWConnection
    private let queue: DispatchQueue
    private let cameraId: DeviceId
    private let localHello: Hello

    private var phase = Phase.awaitingHostHello
    private var parser = RecordParser()
    private var sealer: Sealer?
    private var opener: Opener?

    private let startedAt = monotonicNow()
    private var queuedBytes = 0
    private var lastWriteProgress = monotonicNow()
    private var lastPingAt = monotonicNow()
    private var pingSentAt: TimeInterval?
    private var droppingVideo = false

    init(connection: NWConnection, queue: DispatchQueue, cameraId: DeviceId, localHello: Hello) {
        self.connection = connection
        self.queue = queue
        self.cameraId = cameraId
        self.localHello = localHello
    }

    var isClosed: Bool {
        if case .closed = phase {
            return true
        }
        return false
    }

    var isEstablished: Bool {
        if case .established = phase {
            return true
        }
        return false
    }

    /// Video was dropped and the link has room again, so the stream needs a fresh keyframe.
    var needsKeyframe: Bool {
        droppingVideo && queuedBytes <= videoQueueLimit
    }

    func start() {
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else {
                return
            }
            switch state {
            case .failed(let error):
                self.close(.failed(String(describing: error)))
            case .cancelled:
                self.close(.peerClosed)
            default:
                break
            }
        }
        connection.start(queue: queue)
        receive()
    }

    /// Lets messages through to the delegate. Called once the host is trusted: straight after
    /// authentication when resuming, after the user accepts when pairing.
    func establish() {
        if case .authenticated = phase {
            phase = .established
        }
    }

    /// Sends a message. Messages are dropped silently once the session is closed.
    func send(_ message: Message) {
        do {
            sendSealed(try message.encode())
        } catch {
            EngineLog.shared.warning("session", "could not encode a message: \(error)")
        }
    }

    /// Sends an already encoded `videoFrame` message, applying the flow control from the
    /// specification. Returns `false` if the frame was dropped.
    func sendVideoFrame(_ message: Data, isKeyframe: Bool) -> Bool {
        guard isEstablished else {
            return false
        }
        if droppingVideo {
            guard isKeyframe, queuedBytes <= videoQueueLimit else {
                return false
            }
            droppingVideo = false
        } else if queuedBytes > videoQueueLimit {
            droppingVideo = true
            return false
        }
        sendSealed(message)
        return true
    }

    /// Says goodbye and closes once the goodbye has been written.
    func sendBye(reason: String) {
        guard sealer != nil, !isClosed else {
            close(.localBye(reason))
            return
        }
        do {
            let body = try seal(try Message.bye(Bye(reason: reason)).encode())
            transmit(body) { [weak self] in
                self?.close(.localBye(reason))
            }
        } catch {
            close(.localBye(reason))
        }
    }

    /// Called about once a second by the engine to enforce the timeouts.
    func tick(now: TimeInterval) {
        switch phase {
        case .closed:
            return
        case .awaitingHostHello, .awaitingHostKey, .awaitingPeerHello:
            if now - startedAt > HostSession.handshakeTimeout {
                close(.timeout)
            }
            return
        case .authenticated, .established:
            break
        }

        if queuedBytes > 0, now - lastWriteProgress > HostSession.writeStallTimeout {
            close(.timeout)
            return
        }
        // While the user compares the pairing code, the host only waits for `pairResult` and does
        // not answer pings (see `connect` in `crates/engine/src/session.rs`), so pinging starts
        // once the session is established. The engine's pairing deadline bounds the wait.
        guard isEstablished else {
            return
        }
        if let pingSentAt {
            if now - pingSentAt > HostSession.pongTimeout {
                close(.timeout)
            }
        } else if now - lastPingAt >= HostSession.pingInterval {
            lastPingAt = now
            pingSentAt = now
            send(.ping(UInt64(now * 1_000_000)))
        }
    }

    func close(_ reason: CloseReason) {
        guard !isClosed else {
            return
        }
        phase = .closed
        connection.stateUpdateHandler = nil
        connection.cancel()
        delegate?.hostSession(self, didCloseWith: reason)
    }

    // MARK: - Receiving

    private func receive() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: HostSession.receiveChunk) {
            [weak self] data, _, isComplete, error in
            guard let self, !self.isClosed else {
                return
            }
            if let data, !data.isEmpty {
                self.parser.append(data)
                do {
                    while !self.isClosed, let record = try self.parser.next() {
                        try self.handle(record: record)
                    }
                } catch {
                    self.close(.failed(String(describing: error)))
                    return
                }
            }
            if self.isClosed {
                return
            }
            if let error {
                self.close(.failed(String(describing: error)))
                return
            }
            if isComplete {
                self.close(.peerClosed)
                return
            }
            self.receive()
        }
    }

    private func handle(record: Data) throws {
        switch phase {
        case .awaitingHostHello:
            try handleHostHello(record)
        case .awaitingHostKey(let handshake, let pairingKey):
            let keys = try handshake.finish(hostKey: record, pairingKey: pairingKey)
            self.keys = keys
            sealer = Sealer(key: keys.cameraToHost)
            opener = Opener(key: keys.hostToCamera)
            phase = .awaitingPeerHello
            send(.hello(localHello))
        case .awaitingPeerHello, .authenticated, .established:
            guard var opener else {
                throw ProtocolError.decrypt
            }
            let plaintext = try opener.open(record)
            self.opener = opener
            handle(plaintext: plaintext)
        case .closed:
            break
        }
    }

    private func handleHostHello(_ record: Data) throws {
        let hello: HostHello
        do {
            hello = try HostHello(record: record)
        } catch ProtocolError.unsupportedVersion(let version) {
            EngineLog.shared.warning("session", "host speaks protocol version \(version)")
            refuse(.unsupportedVersion)
            return
        }
        hostHello = hello

        guard let delegate else {
            close(.failed("no delegate"))
            return
        }
        switch delegate.hostSession(self, admit: hello) {
        case .refuse(let status):
            refuse(status)
        case .accept(let pairingKey):
            let handshake = CameraHandshake(host: hello, cameraId: cameraId)
            phase = .awaitingHostKey(handshake, pairingKey: pairingKey)
            transmit(handshake.hello)
        }
    }

    private func refuse(_ status: HandshakeStatus) {
        transmit(CameraHandshake.refusal(status: status, cameraId: cameraId)) { [weak self] in
            self?.close(.refused(status))
        }
    }

    private func handle(plaintext: Data) {
        let message: Message
        do {
            message = try Message.decode(plaintext)
        } catch {
            // A message this version cannot parse is skipped; only a failed decryption is fatal.
            EngineLog.shared.warning("session", "skipped a malformed message: \(error)")
            return
        }

        switch message {
        case .ping(let timestamp):
            send(.pong(timestamp))
        case .pong:
            pingSentAt = nil
        case .bye(let bye):
            close(.peerBye(bye.reason))
        case .hello(let hello):
            guard case .awaitingPeerHello = phase else {
                return
            }
            peerHello = hello
            phase = .authenticated
            delegate?.hostSessionDidAuthenticate(self)
        default:
            // Until the session is established (the user accepted, when pairing), the camera
            // ignores every other message.
            if isEstablished {
                delegate?.hostSession(self, didReceive: message)
            }
        }
    }

    // MARK: - Sending

    private func seal(_ message: Data) throws -> Data {
        guard var sealer else {
            throw ProtocolError.decrypt
        }
        let body = try sealer.seal(message)
        self.sealer = sealer
        return body
    }

    private func sendSealed(_ message: Data) {
        guard !isClosed else {
            return
        }
        do {
            transmit(try seal(message))
        } catch {
            close(.failed(String(describing: error)))
        }
    }

    /// Writes one record. `completion` runs once the bytes have been handed to the network.
    private func transmit(_ body: Data, completion: (() -> Void)? = nil) {
        let framed: Data
        do {
            framed = try RecordFraming.frame(body)
        } catch {
            close(.failed(String(describing: error)))
            return
        }
        if queuedBytes == 0 {
            lastWriteProgress = monotonicNow()
        }
        queuedBytes += framed.count
        let byteCount = framed.count
        connection.send(
            content: framed,
            completion: .contentProcessed { [weak self] error in
                guard let self else {
                    return
                }
                self.queuedBytes -= byteCount
                self.lastWriteProgress = monotonicNow()
                if let error {
                    self.close(.failed(String(describing: error)))
                    return
                }
                completion?()
            }
        )
    }
}
