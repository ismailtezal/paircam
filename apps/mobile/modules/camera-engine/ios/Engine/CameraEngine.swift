import AVFoundation
import Foundation
import Network
import UIKit

/// The camera engine: advertises the phone, pairs with a computer, and streams to it.
///
/// One instance serves the whole app. All engine state lives on a private serial queue; the
/// public methods may be called from any thread and the callbacks may arrive on any thread.
final class CameraEngine {
    static let shared = CameraEngine()

    /// Reasons the camera gives in `bye`, and reports to the interface when a session ends.
    enum EndReason {
        static let stopped = "stopped"
        static let declined = "declined"
        static let pairingTimedOut = "pairingTimedOut"
        static let backgrounded = "backgrounded"
        static let forgotten = "forgotten"
        static let hostEnded = "hostEnded"
        static let lost = "lost"
    }

    private static let pairingTimeout: TimeInterval = 60
    private static let listenerRetryDelay: TimeInterval = 2
    private static let keyframeRequestInterval: TimeInterval = 0.25
    private static let minVideoQueueBytes = 256 * 1024
    /// The send queue may hold this many seconds of video before frames are dropped.
    private static let videoQueueSeconds = 0.25

    /// What the interface listens to. Set from the module's thread and called from the engine's,
    /// so the closures are kept together behind a lock.
    struct Callbacks {
        var onStateChange: ((EngineState) -> Void)?
        var onEnvironmentChange: ((EngineEnvironment) -> Void)?
        var onStats: (([String: Any]) -> Void)?
        var onDisconnect: (([String: Any]) -> Void)?
        var onGesture: (([String: Any]) -> Void)?
    }

    var callbacks: Callbacks {
        get {
            snapshotLock.lock()
            defer { snapshotLock.unlock() }
            return lockedCallbacks
        }
        set {
            snapshotLock.lock()
            lockedCallbacks = newValue
            snapshotLock.unlock()
        }
    }

    let capture = CaptureController()

    private let queue: DispatchQueue
    private let store = PairingStore()
    private let deviceStatus = DeviceStatus()
    private let pathMonitor = NWPathMonitor()
    private let listener: ListenerService

    private let snapshotLock = NSLock()
    private var lockedState = EngineState.idle
    private var lockedEnvironment = EngineEnvironment()
    private var lockedCallbacks = Callbacks()

    // Engine queue state.
    private var state = EngineState.idle
    private var environment = EngineEnvironment()
    private var isRunning = false
    private var isSuspended = false
    private var cameraId: DeviceId?
    private var sessions: [ObjectIdentifier: HostSession] = [:]
    private var active: HostSession?
    private var pairingDeadline: TimeInterval?
    private var isStreaming = false
    private var request = CaptureController.Request()
    private var ticker: DispatchSourceTimer?
    private var lastKeyframeRequest: TimeInterval = 0
    /// What the user chose in the phone's settings, applied when a session starts.
    private var preferredFaceTracking = false
    private var preferredGestures = false
    /// Whether the last `subject` had a face, so only changes are logged.
    private var subjectInView = false

    private var windowStart = monotonicNow()
    private var windowFrames = 0
    private var windowBytes = 0
    private var droppedFrames = 0

    private var lifecycleObservers: [NSObjectProtocol] = []

    private init() {
        let queue = DispatchQueue(label: "app.paircam.engine")
        self.queue = queue
        listener = ListenerService(queue: queue)
        listener.onEvent = { [weak self] event in
            self?.handle(listenerEvent: event)
        }
        listener.onConnection = { [weak self] connection in
            self?.accept(connection)
        }

        capture.onEncodedFrame = { [weak self] output in
            self?.queue.async {
                self?.handle(encoded: output)
            }
        }
        capture.onInterruption = { [weak self] interrupted in
            self?.queue.async {
                self?.updateEnvironment { $0.cameraInterrupted = interrupted }
            }
        }
        capture.onControlsChange = { [weak self] controls in
            self?.queue.async {
                self?.handle(controlsChange: controls)
            }
        }
        capture.onSubject = { [weak self] subject in
            self?.queue.async {
                self?.handle(subject: subject)
            }
        }
        capture.onGesture = { [weak self] gesture in
            self?.queue.async {
                self?.handle(gesture: gesture)
            }
        }

        pathMonitor.pathUpdateHandler = { [weak self] path in
            let reachable = path.status == .satisfied
                && (path.usesInterfaceType(.wifi) || path.usesInterfaceType(.wiredEthernet))
            self?.updateEnvironment { $0.wifi = reachable }
        }
        pathMonitor.start(queue: queue)

        let center = NotificationCenter.default
        lifecycleObservers.append(
            center.addObserver(
                forName: UIApplication.didEnterBackgroundNotification,
                object: nil,
                queue: nil
            ) { [weak self] _ in
                self?.queue.async {
                    self?.suspend()
                }
            }
        )
        lifecycleObservers.append(
            center.addObserver(
                forName: UIApplication.willEnterForegroundNotification,
                object: nil,
                queue: nil
            ) { [weak self] _ in
                self?.queue.async {
                    self?.resume()
                }
            }
        )
    }

    // MARK: - Public API

    var stateSnapshot: EngineState {
        snapshotLock.lock()
        defer { snapshotLock.unlock() }
        return lockedState
    }

    var environmentSnapshot: EngineEnvironment {
        snapshotLock.lock()
        defer { snapshotLock.unlock() }
        return lockedEnvironment
    }

    /// Starts advertising and the camera. Calling it again is harmless and picks up a camera
    /// permission that was granted in the meantime.
    func start() {
        queue.async {
            self.refreshPermission()
            self.capture.start()
            guard !self.isRunning else {
                return
            }
            self.isRunning = true
            self.isSuspended = false
            self.startListening()
            self.startTicker()
            self.setState(.advertising)
        }
    }

    func stop() {
        queue.async {
            guard self.isRunning else {
                return
            }
            self.isRunning = false
            self.shutDown(reason: EndReason.stopped)
            self.ticker?.cancel()
            self.ticker = nil
        }
    }

    /// The user's answer to a pairing request.
    func respondToPairing(accept: Bool) {
        queue.async {
            guard
                let session = self.active,
                case .pairing(let hostName, _) = self.state,
                let hostHello = session.hostHello,
                let keys = session.keys
            else {
                return
            }
            self.pairingDeadline = nil
            session.send(.pairResult(PairResult(accepted: accept)))
            if accept {
                if !self.store.save(host: hostHello.hostId, name: hostName, key: keys.pairingKey) {
                    EngineLog.shared.error("engine", "could not save the pairing key")
                }
                self.establish(session, hostName: hostName)
            } else {
                session.sendBye(reason: EndReason.declined)
            }
        }
    }

    /// Ends the current session at the user's request and goes back to waiting.
    func disconnect() {
        queue.async {
            self.active?.sendBye(reason: EndReason.stopped)
        }
    }

    func pairedHosts(completion: @escaping ([[String: Any]]) -> Void) {
        queue.async {
            let hosts = self.store.hosts().map { host -> [String: Any] in
                var entry: [String: Any] = ["id": host.id.hex, "name": host.name]
                if let pairedAt = host.pairedAt {
                    entry["pairedAt"] = pairedAt.timeIntervalSince1970 * 1000
                }
                return entry
            }
            completion(hosts)
        }
    }

    func forgetHost(id: String, completion: @escaping () -> Void) {
        queue.async {
            if let hostId = DeviceId(hex: id) {
                self.store.remove(host: hostId)
                if let session = self.active, session.hostHello?.hostId == hostId {
                    session.sendBye(reason: EndReason.forgotten)
                }
            }
            completion()
        }
    }

    /// The user's choice of face tracking and hand gestures in the phone's settings. `nil` leaves
    /// a feature as it is. Saved for the next session, and applied at once to a session that is
    /// already running.
    func setVisionPreferences(faceTracking: Bool?, gestures: Bool?) {
        queue.async {
            if let faceTracking {
                self.preferredFaceTracking = faceTracking
            }
            if let gestures {
                self.preferredGestures = gestures
            }
            guard let session = self.active, session.isEstablished else {
                return
            }
            self.capture.applyControl(ControlValues(faceTracking: faceTracking, gestures: gestures)) {
                [weak self, weak session] controls in
                self?.queue.async {
                    guard let self, let session, session === self.active else {
                        return
                    }
                    session.send(.controlState(controls))
                    self.publishVision(controls)
                }
            }
        }
    }

    /// Re-reads the camera permission after the user answered the system prompt, and turns the
    /// camera on so the preview is live straight away. The network service is not touched:
    /// advertising waits for `start()`.
    func permissionDidChange() {
        queue.async {
            self.refreshPermission()
            if !self.isSuspended {
                self.capture.start()
            }
        }
    }

    // MARK: - Lifecycle

    private func startListening() {
        let id = cameraId ?? store.deviceId()
        cameraId = id
        let name = deviceStatus.deviceName
        updateEnvironment { $0.deviceName = name }
        listener.start(name: name, cameraId: id)
    }

    private func startTicker() {
        ticker?.cancel()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + .seconds(1), repeating: .seconds(1), leeway: .milliseconds(100))
        timer.setEventHandler { [weak self] in
            self?.tick()
        }
        timer.resume()
        ticker = timer
    }

    /// iOS stops the camera in the background, so the engine says goodbye and waits.
    private func suspend() {
        guard isRunning, !isSuspended else {
            return
        }
        isSuspended = true
        shutDown(reason: EndReason.backgrounded)
    }

    private func resume() {
        refreshPermission()
        guard isRunning, isSuspended else {
            return
        }
        isSuspended = false
        startListening()
        capture.start()
        setState(.advertising)
    }

    private func shutDown(reason: String) {
        listener.stop()
        capture.stop()
        updateEnvironment {
            $0.port = 0
            $0.cameraInterrupted = false
        }
        for session in Array(sessions.values) where session !== active {
            session.close(.localBye(reason))
        }
        if let active {
            active.sendBye(reason: reason)
        } else {
            setState(.idle)
        }
    }

    private func refreshPermission() {
        let permission = EngineEnvironment.cameraPermissionName()
        updateEnvironment { $0.cameraPermission = permission }
    }

    // MARK: - Listener

    private func handle(listenerEvent event: ListenerService.Event) {
        switch event {
        case .ready(let port):
            EngineLog.shared.info("listener", "listening on port \(port)")
            updateEnvironment { $0.port = Int(port) }
        case .advertised:
            updateEnvironment { $0.localNetwork = .granted }
        case .localNetworkDenied:
            EngineLog.shared.warning("listener", "local network access was denied")
            updateEnvironment { $0.localNetwork = .denied }
        case .failed(let description):
            EngineLog.shared.error("listener", "failed: \(description)")
            updateEnvironment { $0.port = 0 }
            queue.asyncAfter(deadline: .now() + CameraEngine.listenerRetryDelay) { [weak self] in
                guard let self, self.isRunning, !self.isSuspended else {
                    return
                }
                self.startListening()
            }
        }
    }

    private func accept(_ connection: NWConnection) {
        guard isRunning, !isSuspended, let cameraId else {
            connection.cancel()
            return
        }
        let hello = Hello(
            name: deviceStatus.deviceName,
            appVersion: DeviceStatus.appVersion,
            platform: "ios",
            model: DeviceStatus.model
        )
        let session = HostSession(connection: connection, queue: queue, cameraId: cameraId, localHello: hello)
        session.delegate = self
        sessions[ObjectIdentifier(session)] = session
        session.start()
    }

    // MARK: - Sessions

    private func establish(_ session: HostSession, hostName: String) {
        session.establish()
        session.videoQueueLimit = videoQueueLimit()
        setState(.connected(hostName: hostName))
        EngineLog.shared.info("engine", "connected to \(hostName)")

        EngineLog.shared.attach { [weak self, weak session] entry in
            self?.queue.async {
                guard let session, session.isEstablished else {
                    return
                }
                session.send(.log(entry))
            }
        }
        // The user's choices from the phone's settings, in place before the first controlState.
        subjectInView = false
        capture.applyControl(
            ControlValues(faceTracking: preferredFaceTracking, gestures: preferredGestures)
        ) { _ in }
        sendCapabilities(to: session)
    }

    /// Sends `capabilities` followed by `controlState`, in that order.
    private func sendCapabilities(to session: HostSession) {
        capture.capabilities { [weak self, weak session] capabilities in
            self?.queue.async {
                session?.send(.capabilities(capabilities))
            }
        }
        capture.controlState { [weak self, weak session] controls in
            self?.queue.async {
                session?.send(.controlState(controls))
                self?.publishVision(controls)
            }
        }
    }

    private func handle(_ message: Message, from session: HostSession) {
        guard session === active, let hostName = state.hostName else {
            return
        }
        switch message {
        case .configure(let configure):
            request = CaptureController.Request(configure)
            session.videoQueueLimit = videoQueueLimit()
            capture.apply(request) { [weak self, weak session] format in
                self?.queue.async {
                    guard let self, let session, session === self.active else {
                        return
                    }
                    EngineLog.shared.info(
                        "engine",
                        "configured \(format.camera) \(format.width)x\(format.height) at \(format.fps) fps"
                    )
                    if format.cameraChanged {
                        self.sendCapabilities(to: session)
                    }
                    if self.isStreaming {
                        self.setStreamingState(hostName: hostName, format: format)
                    }
                }
            }
        case .startStream:
            guard !isStreaming else {
                return
            }
            isStreaming = true
            droppedFrames = 0
            resetStatsWindow()
            capture.activeFormat { [weak self, weak session] format in
                self?.queue.async {
                    guard let self, let session, session === self.active, self.isStreaming else {
                        return
                    }
                    self.setStreamingState(hostName: hostName, format: format)
                }
            }
            capture.startEncoding()
        case .stopStream:
            guard isStreaming else {
                return
            }
            isStreaming = false
            capture.stopEncoding()
            setState(.connected(hostName: hostName))
        case .requestKeyframe:
            capture.requestKeyframe()
        case .control(let values):
            if let faceTracking = values.faceTracking {
                EngineLog.shared.info("vision", "\(hostName) turned face tracking \(faceTracking ? "on" : "off")")
            }
            if let gestures = values.gestures {
                EngineLog.shared.info("vision", "\(hostName) turned gestures \(gestures ? "on" : "off")")
            }
            capture.applyControl(values) { [weak self, weak session] controls in
                self?.queue.async {
                    session?.send(.controlState(controls))
                    self?.publishVision(controls)
                }
            }
        default:
            break
        }
    }

    private func setStreamingState(hostName: String, format: CaptureController.ActiveFormat) {
        // Keep the encoded size once the encoder has reported it: it accounts for rotation.
        if case .streaming(_, let width, let height, _, let codec) = state, codec == format.codec {
            let sameShape = max(width, height) == max(format.width, format.height)
                && min(width, height) == min(format.width, format.height)
            if sameShape {
                setState(.streaming(hostName: hostName, width: width, height: height, fps: format.fps, codec: codec))
                return
            }
        }
        setState(
            .streaming(
                hostName: hostName,
                width: format.width,
                height: format.height,
                fps: format.fps,
                codec: format.codec
            )
        )
    }

    private func handle(encoded output: VideoEncoder.Output) {
        guard isStreaming, let session = active, session.isEstablished else {
            return
        }
        if let config = output.config {
            session.send(.videoConfig(config))
            if case .streaming(let hostName, _, _, let fps, _) = state {
                setState(
                    .streaming(
                        hostName: hostName,
                        width: Int(config.width),
                        height: Int(config.height),
                        fps: fps,
                        codec: config.codec
                    )
                )
            }
        }

        if session.sendVideoFrame(output.message, isKeyframe: output.isKeyframe) {
            windowFrames += 1
            windowBytes += output.payloadBytes
        } else {
            droppedFrames += 1
        }

        let now = monotonicNow()
        if session.needsKeyframe, now - lastKeyframeRequest > CameraEngine.keyframeRequestInterval {
            lastKeyframeRequest = now
            capture.requestKeyframe()
        }
    }

    // MARK: - Face tracking and gestures

    /// A gesture acted or a zoom ramp settled: the host hears the new state of every control.
    private func handle(controlsChange controls: ControlValues) {
        publishVision(controls)
        guard let session = active, session.isEstablished else {
            return
        }
        session.send(.controlState(controls))
    }

    private func handle(subject: SubjectUpdate) {
        guard let session = active, session.isEstablished, environment.faceTracking else {
            return
        }
        let inView = subject.face != nil
        if inView != subjectInView {
            subjectInView = inView
            EngineLog.shared.info("vision", inView ? "face found" : "face lost")
        }
        session.send(.subject(subject))
    }

    private func handle(gesture: GestureEvent) {
        guard let session = active, session.isEstablished else {
            return
        }
        session.send(.gesture(gesture))
        callbacks.onGesture?(["kind": gesture.kind.rawValue, "phase": gesture.phase.rawValue])
    }

    /// Keeps the interface's view of face tracking and gestures in step with the camera.
    private func publishVision(_ controls: ControlValues) {
        updateEnvironment {
            if let faceTracking = controls.faceTracking {
                $0.faceTracking = faceTracking
            }
            if let gestures = controls.gestures {
                $0.gestures = gestures
            }
        }
    }

    private func videoQueueLimit() -> Int {
        let seconds = Double(request.bitrate) / 8 * CameraEngine.videoQueueSeconds
        return max(CameraEngine.minVideoQueueBytes, Int(seconds))
    }

    // MARK: - Ticker

    private func tick() {
        let now = monotonicNow()
        for session in Array(sessions.values) {
            session.tick(now: now)
        }

        if let deadline = pairingDeadline, now > deadline, let session = active {
            pairingDeadline = nil
            session.send(.pairResult(PairResult(accepted: false)))
            session.sendBye(reason: EndReason.pairingTimedOut)
        }

        if isStreaming, let session = active, session.isEstablished {
            sendStats(to: session, now: now)
        }
    }

    private func sendStats(to session: HostSession, now: TimeInterval) {
        let elapsed = max(now - windowStart, 0.001)
        let fps = (Double(windowFrames) / elapsed * 100).rounded() / 100
        let stats = Stats(
            fps: fps,
            bitrate: Int(Double(windowBytes) * 8 / elapsed),
            droppedFrames: droppedFrames,
            thermal: deviceStatus.thermal,
            battery: (deviceStatus.batteryLevel * 100).rounded() / 100,
            charging: deviceStatus.isCharging
        )
        resetStatsWindow()

        session.send(.stats(stats))
        callbacks.onStats?([
            "fps": stats.fps,
            "bitrate": stats.bitrate,
            "droppedFrames": stats.droppedFrames,
            "thermal": stats.thermal.rawValue,
            "battery": stats.battery,
            "charging": stats.charging,
        ])
    }

    private func resetStatsWindow() {
        windowStart = monotonicNow()
        windowFrames = 0
        windowBytes = 0
    }

    // MARK: - State

    private func setState(_ newState: EngineState) {
        guard newState != state else {
            return
        }
        state = newState
        snapshotLock.lock()
        lockedState = newState
        snapshotLock.unlock()
        callbacks.onStateChange?(newState)
    }

    private func updateEnvironment(_ change: (inout EngineEnvironment) -> Void) {
        var updated = environment
        change(&updated)
        guard updated != environment else {
            return
        }
        environment = updated
        snapshotLock.lock()
        lockedEnvironment = updated
        snapshotLock.unlock()
        callbacks.onEnvironmentChange?(updated)
    }
}

// MARK: - HostSessionDelegate

extension CameraEngine: HostSessionDelegate {
    func hostSession(_ session: HostSession, admit hello: HostHello) -> HostSession.Admission {
        // One host at a time.
        if let active, active !== session {
            EngineLog.shared.info("engine", "turned away host \(hello.hostId.hex): busy")
            return .refuse(.busy)
        }
        switch hello.mode {
        case .resume:
            guard let key = store.pairingKey(for: hello.hostId) else {
                EngineLog.shared.info("engine", "turned away host \(hello.hostId.hex): not paired")
                return .refuse(.notPaired)
            }
            active = session
            return .accept(pairingKey: key)
        case .pair:
            active = session
            return .accept(pairingKey: CameraHandshake.unpaired)
        }
    }

    func hostSessionDidAuthenticate(_ session: HostSession) {
        guard
            session === active,
            let hostHello = session.hostHello,
            let peerHello = session.peerHello,
            let keys = session.keys
        else {
            return
        }
        let trimmed = peerHello.name.trimmingCharacters(in: .whitespacesAndNewlines)
        let hostName = trimmed.isEmpty ? hostHello.hostId.hex : String(trimmed.prefix(64))

        switch hostHello.mode {
        case .resume:
            // Only a paired host can produce a hello the camera can decrypt.
            store.rename(host: hostHello.hostId, to: hostName)
            establish(session, hostName: hostName)
        case .pair:
            pairingDeadline = monotonicNow() + CameraEngine.pairingTimeout
            setState(.pairing(hostName: hostName, code: keys.pairingCodeDigits))
        }
    }

    func hostSession(_ session: HostSession, didReceive message: Message) {
        handle(message, from: session)
    }

    func hostSession(_ session: HostSession, didCloseWith reason: HostSession.CloseReason) {
        sessions.removeValue(forKey: ObjectIdentifier(session))
        guard session === active else {
            return
        }

        let previous = state
        active = nil
        pairingDeadline = nil
        if isStreaming {
            isStreaming = false
            capture.stopEncoding()
        }
        // Face tracking and gestures belong to a session; the next one starts from the settings.
        capture.applyControl(ControlValues(faceTracking: false, gestures: false)) { _ in }
        subjectInView = false
        updateEnvironment {
            $0.faceTracking = false
            $0.gestures = false
        }
        EngineLog.shared.detach()
        EngineLog.shared.info("engine", "session ended: \(reason)")
        setState(isRunning && !isSuspended ? .advertising : .idle)

        guard let hostName = previous.hostName else {
            return
        }
        let ending: String
        switch reason {
        case .refused:
            return
        case .peerBye:
            ending = EndReason.hostEnded
        case .localBye(let localReason):
            ending = localReason
        case .peerClosed, .timeout, .failed:
            ending = EndReason.lost
        }
        var event: [String: Any] = ["reason": ending, "hostName": hostName]
        switch previous {
        case .pairing:
            event["during"] = "pairing"
        case .streaming:
            event["during"] = "streaming"
        default:
            event["during"] = "connected"
        }
        callbacks.onDisconnect?(event)
    }
}
