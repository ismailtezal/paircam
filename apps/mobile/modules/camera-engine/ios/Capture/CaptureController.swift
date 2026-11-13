import AVFoundation
import UIKit

/// The camera: one capture session shared by the on-screen preview and the encoder.
///
/// The session runs whenever the app is in the foreground so the preview is always live. Frames
/// reach the encoder only while a computer is streaming. Session and device changes happen on a
/// private session queue; frames are delivered and encoded on a private video queue.
///
/// In Cinematic mode (iOS 26) the phone films from the camera on the chosen side that can, with
/// Apple's Cinematic Video capture on: the frames arrive with the lens blur already in them, a
/// metadata output reports what the camera could focus on, and the desktop moves the focus.
final class CaptureController: NSObject {
    /// What the desktop asked for, or the defaults until it asks.
    struct Request: Equatable {
        var camera = CameraCatalog.defaultCameraId
        var width = 1920
        var height = 1080
        var fps = 30
        var codec = VideoCodec.hevc
        var bitrate = 12_000_000
        var keyframeInterval = 2.0

        init() {}

        /// Takes the desktop's request, keeping every number inside a range the camera and the
        /// encoder can be asked for without trapping.
        init(_ configure: Configure) {
            camera = configure.camera
            width = min(max(configure.width, 1), 16_384)
            height = min(max(configure.height, 1), 16_384)
            fps = min(max(configure.fps, 1), 240)
            codec = configure.codec
            bitrate = min(max(configure.bitrate, 100_000), 400_000_000)
            keyframeInterval = min(max(configure.keyframeInterval, 0.1), 3600)
        }
    }

    /// What the camera is really doing after a request was applied.
    struct ActiveFormat: Equatable {
        var camera: String
        var width: Int
        var height: Int
        var fps: Int
        var codec: VideoCodec
        /// The request named a different camera than the one that was active.
        var cameraChanged: Bool
    }

    /// Posted on the main thread when the preview should follow a different camera.
    static let activeDeviceDidChange = Notification.Name("app.paircam.activeDeviceDidChange")

    /// The highest zoom offered, as the user sees it (1× is the wide lens).
    private static let maxZoom: CGFloat = 10
    /// Zoom ramp speeds, in doublings per second. A change from the desktop ramps at
    /// `zoomRampRate`, the `frame` gesture a little slower so it reads as a deliberate move, and
    /// `spread` faster so the picture keeps up with the hands.
    private static let zoomRampRate: Float = 3
    private static let frameZoomRampRate: Float = 2
    /// Face tracking is switched off for now: turning it on froze the picture on a real phone.
    /// While this is false the phone does not announce it, ignores requests for it and ignores
    /// the open-palm gesture that toggles it.
    private static let faceTrackingAvailable = false
    private static let spreadZoomRampRate: Float = 6
    /// How often a running zoom ramp is checked to see whether it has settled.
    private static let zoomSettleCheckInterval: TimeInterval = 0.1
    /// What Cinematic mode sees is reported at most this often, unless the focus moves.
    private static let cinematicReportInterval: TimeInterval = 0.1
    /// Without metadata for this long, nothing is in view any more.
    private static let cinematicQuietInterval: TimeInterval = 0.5

    let session = AVCaptureSession()

    /// Compressed frames, delivered on a VideoToolbox thread.
    var onEncodedFrame: ((VideoEncoder.Output) -> Void)?
    /// The camera was taken away (`true`) or given back (`false`). Any thread.
    var onInterruption: ((Bool) -> Void)?
    /// The controls changed without a request from the desktop: a gesture acted, or a zoom ramp
    /// settled. Carries the state of every control. Any thread.
    var onControlsChange: ((ControlValues) -> Void)?
    /// Where the person is, while face tracking is on. Any thread.
    var onSubject: ((SubjectUpdate) -> Void)?
    /// A hand gesture the camera recognised and acted on. Any thread.
    var onGesture: ((GestureEvent) -> Void)?
    /// What Cinematic mode sees and where it focuses, while it is on, and an empty state once
    /// when it goes off. Any thread.
    var onCinematic: ((CinematicState) -> Void)?

    private let sessionQueue = DispatchQueue(label: "app.paircam.capture.session")
    private let videoQueue = DispatchQueue(label: "app.paircam.capture.video", qos: .userInteractive)
    private let videoOutput = AVCaptureVideoDataOutput()
    private let rotation = CaptureRotation()
    private let encoder: VideoEncoder
    private let analyzer = VisionAnalyzer()
    private let metadataOutput = AVCaptureMetadataOutput()
    private let metadataQueue = DispatchQueue(label: "app.paircam.capture.metadata")

    // Session queue state.
    private var isConfigured = false
    private var shouldRun = false
    private var cameras: [CameraDescriptor] = []
    private var activeCamera: CameraDescriptor?
    private var input: AVCaptureDeviceInput?
    private var request = Request()
    private var focusMode = FocusMode.auto
    private var stabilization = StabilizationMode.off
    private var faceTracking = false
    private var gestures = false
    /// The device zoom factor a running ramp is heading for; `nil` when no ramp is running.
    private var zoomTarget: CGFloat?
    /// Bumped by every ramp, so only the latest one reports when it settles.
    private var zoomGeneration = 0
    /// The zoom, as the user sees it, when the current spread gesture began.
    private var spreadStartZoom: Double?
    /// The camera the desktop chose. In Cinematic mode `activeCamera` is the camera on the
    /// same side that films it.
    private var logicalCamera: CameraDescriptor?
    /// The Cinematic camera on each side, found once.
    private var cinematicCameras: [CameraPosition: CameraDescriptor] = [:]
    /// What the phone can do in Cinematic mode, or `nil` when it cannot.
    private var cinematicSupport: CinematicCapabilities?
    /// The desktop asked for Cinematic mode.
    private var cinematicRequested = false
    /// The camera is filming in Cinematic mode.
    private var cinematicActive = false
    /// The simulated aperture as an f-number: the user's choice, or the format's default.
    private var aperture: Double?
    /// An open palm pulled the focus to the hand; the next one hands it back.
    private var palmFocusHeld = false
    private var sceneObservation: NSKeyValueObservation?

    // Metadata queue state.
    private var cinematicSubjects: [CinematicSubject] = []
    private var lowLight = false
    private var lastCinematicReport: (time: TimeInterval, state: CinematicState)?
    /// Bumped by every metadata delivery, so a quiet check knows whether one came since.
    private var metadataGeneration = 0

    /// The subjects as last seen, for focus commands on the session queue.
    private let subjectsLock = NSLock()
    private var seenSubjects: [CinematicSubject] = []

    // Video queue state.
    private var isEncoding = false

    private let deviceLock = NSLock()
    private var lockedDevice: AVCaptureDevice?

    override init() {
        let request = Request()
        encoder = VideoEncoder(
            settings: VideoEncoder.Settings(
                codec: request.codec,
                bitrate: request.bitrate,
                fps: request.fps,
                keyframeInterval: request.keyframeInterval
            )
        )
        super.init()

        encoder.onOutput = { [weak self] output in
            self?.onEncodedFrame?(output)
        }
        rotation.onChange = { [weak self] in
            self?.sessionQueue.async {
                self?.applyRotation()
            }
        }
        analyzer.onFace = { [weak self] face in
            self?.onSubject?(SubjectUpdate(face: face))
        }
        analyzer.onGesture = { [weak self] event, point in
            self?.sessionQueue.async {
                self?.handle(gesture: event, at: point)
            }
        }

        let center = NotificationCenter.default
        center.addObserver(
            self,
            selector: #selector(sessionWasInterrupted(_:)),
            name: .AVCaptureSessionWasInterrupted,
            object: session
        )
        center.addObserver(
            self,
            selector: #selector(sessionInterruptionEnded(_:)),
            name: .AVCaptureSessionInterruptionEnded,
            object: session
        )
        center.addObserver(
            self,
            selector: #selector(sessionRuntimeError(_:)),
            name: .AVCaptureSessionRuntimeError,
            object: session
        )
        center.addObserver(
            self,
            selector: #selector(subjectAreaDidChange(_:)),
            name: .AVCaptureDeviceSubjectAreaDidChange,
            object: nil
        )
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
    }

    /// The camera the preview should follow. Safe to read from any thread.
    var currentDevice: AVCaptureDevice? {
        deviceLock.lock()
        defer { deviceLock.unlock() }
        return lockedDevice
    }

    // MARK: - Session

    /// Starts the camera if the app may use it. Does nothing without camera permission.
    func start() {
        sessionQueue.async {
            self.shouldRun = true
            guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
                EngineLog.shared.warning("capture", "camera permission is missing; the camera stays off")
                return
            }
            self.configureIfNeeded()
            if self.isConfigured, !self.session.isRunning {
                self.session.startRunning()
            }
        }
    }

    func stop() {
        sessionQueue.async {
            self.shouldRun = false
            if self.session.isRunning {
                self.session.stopRunning()
            }
        }
    }

    /// Applies what the desktop asked for and reports what the camera is really doing.
    func apply(_ newRequest: Request, completion: @escaping (ActiveFormat) -> Void) {
        sessionQueue.async {
            let previousCamera = self.logicalCamera?.id
            let previous = self.request
            self.request = newRequest
            let wasConfigured = self.isConfigured
            self.configureIfNeeded()
            // A bitrate or keyframe change alone must not disturb the running camera.
            let captureChanged = previous.camera != newRequest.camera
                || previous.width != newRequest.width
                || previous.height != newRequest.height
                || previous.fps != newRequest.fps
            if self.isConfigured, wasConfigured, captureChanged {
                self.reconfigureCamera()
            }
            let settings = self.encoderSettings()
            self.videoQueue.async {
                self.encoder.configure(settings)
            }
            var active = self.activeFormat()
            active.cameraChanged = previousCamera != nil && previousCamera != active.camera
            completion(active)
        }
    }

    func activeFormat(completion: @escaping (ActiveFormat) -> Void) {
        sessionQueue.async {
            completion(self.activeFormat())
        }
    }

    // MARK: - Encoding

    func startEncoding() {
        videoQueue.async {
            self.encoder.invalidate()
            self.isEncoding = true
        }
    }

    func stopEncoding() {
        videoQueue.async {
            self.isEncoding = false
            self.encoder.invalidate()
        }
    }

    func requestKeyframe() {
        videoQueue.async {
            self.encoder.requestKeyframe()
        }
    }

    // MARK: - Capabilities and controls

    func capabilities(completion: @escaping (Capabilities) -> Void) {
        sessionQueue.async {
            self.configureIfNeeded()
            let cameras = self.cameras.map { camera in
                CameraInfo(
                    id: camera.id,
                    name: camera.device.localizedName,
                    position: camera.position,
                    lens: camera.lens,
                    formats: CameraCatalog.formats(for: camera.device)
                )
            }
            completion(
                Capabilities(
                    cameras: cameras,
                    codecs: [.hevc, .h264],
                    controls: self.controlCapabilities()
                )
            )
        }
    }

    func controlState(completion: @escaping (ControlValues) -> Void) {
        sessionQueue.async {
            completion(self.controlState())
        }
    }

    /// Changes the controls named in `values` and reports the state of all of them. A zoom change
    /// ramps; the reported zoom is where the ramp will settle, and `onControlsChange` reports
    /// again once it has.
    func applyControl(_ values: ControlValues, completion: @escaping (ControlValues) -> Void) {
        sessionQueue.async {
            // Cinematic mode can switch cameras, so it goes first and the rest applies to the
            // camera it leaves running.
            if let on = values.cinematic {
                self.setCinematic(on)
            }
            if let aperture = values.aperture {
                self.setAperture(aperture)
            }
            if let device = self.activeCamera?.device {
                do {
                    try device.lockForConfiguration()
                    self.apply(values, to: device)
                    device.unlockForConfiguration()
                } catch {
                    EngineLog.shared.warning("capture", "could not lock the camera for a control change: \(error)")
                }
            }
            if let mode = values.stabilization, mode != .unknown {
                self.stabilization = mode
                self.applyStabilization()
            }
            if values.faceTracking != nil || values.gestures != nil {
                self.setVision(
                    faceTracking: values.faceTracking ?? self.faceTracking,
                    gestures: values.gestures ?? self.gestures
                )
            }
            completion(self.controlState())
        }
    }

    // MARK: - Session queue

    private func configureIfNeeded() {
        guard !isConfigured, AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
            return
        }
        cameras = CameraCatalog.discover()
        guard !cameras.isEmpty else {
            EngineLog.shared.error("capture", "no camera was found")
            return
        }
        for position in [CameraPosition.back, .front] {
            if let camera = CameraCatalog.cinematicCamera(for: position) {
                cinematicCameras[position] = camera
            }
        }
        cinematicSupport = cinematicCapabilities()
        if let support = cinematicSupport {
            EngineLog.shared.info(
                "capture",
                "Cinematic mode available on: \(support.positions.map { $0.rawValue }.joined(separator: ", "))"
            )
        }

        session.beginConfiguration()
        session.automaticallyConfiguresCaptureDeviceForWideColor = false
        videoOutput.alwaysDiscardsLateVideoFrames = true
        videoOutput.videoSettings = [
            kCVPixelBufferPixelFormatTypeKey as String: Int(CameraCatalog.pixelFormat),
        ]
        videoOutput.setSampleBufferDelegate(self, queue: videoQueue)
        if session.canAddOutput(videoOutput) {
            session.addOutput(videoOutput)
        } else {
            EngineLog.shared.error("capture", "the session refused the video output")
        }
        selectCamera()
        session.commitConfiguration()
        configureConnection()
        isConfigured = true
        EngineLog.shared.info("capture", "found cameras: \(cameras.map { $0.id }.joined(separator: ", "))")
    }

    /// Switches to the requested camera, format and frame rate, and in Cinematic mode to the camera
    /// on the same side that films it. Call inside a configuration block.
    private func selectCamera() {
        let requested = cameras.first(where: { $0.id == request.camera })
        let fallback = cameras.first(where: { $0.id == CameraCatalog.defaultCameraId })
        guard let chosen = requested ?? logicalCamera ?? fallback ?? cameras.first else {
            return
        }
        logicalCamera = chosen
        // Settled below once the camera, format and Cinematic capture are in place; until then
        // nothing is filming in Cinematic mode.
        cinematicActive = false
        let cinematicCamera = cinematicRequested ? cinematicCameras[chosen.position] : nil
        if cinematicRequested, cinematicCamera == nil {
            EngineLog.shared.warning("capture", "no \(chosen.position.rawValue) camera can film in Cinematic mode")
        }
        let camera = cinematicCamera ?? chosen

        // A multi-lens camera starts on the wide lens (1×) and keeps its zoom across format
        // changes, rather than falling back to the ultra wide at the device's own 1.
        var displayZoom = 1.0
        if camera.id == activeCamera?.id, input != nil {
            displayZoom = currentZoom(of: camera)
        }
        zoomTarget = nil
        zoomGeneration += 1

        if camera.id != activeCamera?.id || input == nil {
            do {
                let newInput = try AVCaptureDeviceInput(device: camera.device)
                if let input {
                    session.removeInput(input)
                }
                guard session.canAddInput(newInput) else {
                    EngineLog.shared.error("capture", "the session refused camera \(camera.id)")
                    if let input, session.canAddInput(input) {
                        session.addInput(input)
                    }
                    return
                }
                session.addInput(newInput)
                input = newInput
                activeCamera = camera
                focusMode = .auto
                publish(device: camera.device)
            } catch {
                EngineLog.shared.error("capture", "could not open camera \(camera.id): \(error)")
                return
            }
        }

        let device = camera.device
        var candidate: AVCaptureDevice.Format?
        var fps = request.fps
        if cinematicCamera != nil, #available(iOS 26.0, *) {
            candidate = CameraCatalog.cinematicFormat(for: device, width: request.width, height: request.height)
            if let candidate {
                fps = CameraCatalog.cinematicFrameRate(of: candidate, closestTo: request.fps)
            }
        } else {
            candidate = CameraCatalog.bestFormat(
                for: device,
                width: request.width,
                height: request.height,
                fps: request.fps
            )
            if let candidate {
                fps = CameraCatalog.supportedFrameRate(of: candidate, closestTo: request.fps)
            }
        }
        guard let format = candidate else {
            EngineLog.shared.warning("capture", "camera \(camera.id) has no usable format")
            return
        }
        // Zoom limits depend on it, so it is set before the zoom; `setCinematicCapture` settles it.
        cinematicActive = cinematicCamera != nil
        do {
            try device.lockForConfiguration()
            device.activeFormat = format
            let frameDuration = CMTime(value: 1, timescale: CMTimeScale(fps))
            device.activeVideoMinFrameDuration = frameDuration
            device.activeVideoMaxFrameDuration = frameDuration
            if camera.zoomScale != 1 || cinematicActive {
                device.videoZoomFactor = zoomFactor(forDisplay: displayZoom, camera: camera)
            }
            // Cinematic mode decides the focus itself.
            if !cinematicActive, focusMode == .auto, device.isFocusModeSupported(.continuousAutoFocus) {
                device.focusMode = .continuousAutoFocus
            }
            device.isSubjectAreaChangeMonitoringEnabled = !cinematicActive
            device.unlockForConfiguration()
        } catch {
            EngineLog.shared.warning("capture", "could not set the format on \(camera.id): \(error)")
        }
        setCinematicCapture(cinematicCamera != nil)
    }

    /// Turns Cinematic capture on the current input on or off. Afterwards `cinematicActive`
    /// says whether it really is on. Call inside a configuration block, after the format is set.
    private func setCinematicCapture(_ on: Bool) {
        guard #available(iOS 26.0, *), let input else {
            cinematicActive = false
            return
        }
        if on, input.isCinematicVideoCaptureSupported {
            input.isCinematicVideoCaptureEnabled = true
            applyAperture(to: input)
            cinematicActive = true
            return
        }
        if input.isCinematicVideoCaptureEnabled {
            input.isCinematicVideoCaptureEnabled = false
        }
        if on {
            EngineLog.shared.warning("capture", "the camera cannot film this format in Cinematic mode")
        }
        cinematicActive = false
    }

    /// Re-selects the camera with the outputs in a safe order: the metadata output leaves before
    /// Cinematic capture can turn off, and joins once it is on, with the object types Cinematic
    /// capture requires (any other set raises an exception).
    private func reconfigureCamera() {
        detachMetadataOutput()
        session.beginConfiguration()
        selectCamera()
        session.commitConfiguration()
        configureConnection()
        if cinematicActive {
            attachMetadataOutput()
        } else {
            clearCinematic()
        }
    }

    /// Mirroring, rotation and stabilization live on the connection, which is replaced whenever
    /// the input changes.
    private func configureConnection() {
        guard let connection = videoOutput.connection(with: .video) else {
            return
        }
        if connection.isVideoMirroringSupported {
            connection.automaticallyAdjustsVideoMirroring = false
            connection.isVideoMirrored = activeCamera?.position == .front
        }
        applyStabilization()
        applyRotation()
    }

    private func applyRotation() {
        guard let connection = videoOutput.connection(with: .video) else {
            return
        }
        if rotation.apply(to: connection) {
            // A new orientation is a new picture size or a flipped picture: restart the encoder
            // so the host gets a fresh videoConfig followed by a keyframe.
            videoQueue.async {
                self.encoder.invalidate()
            }
        }
    }

    private func applyStabilization() {
        guard
            let connection = videoOutput.connection(with: .video),
            connection.isVideoStabilizationSupported,
            let format = activeCamera?.device.activeFormat
        else {
            return
        }
        let mode = CaptureController.captureMode(for: stabilization)
        if mode == .off || format.isVideoStabilizationModeSupported(mode) {
            connection.preferredVideoStabilizationMode = mode
        } else {
            stabilization = .off
            connection.preferredVideoStabilizationMode = .off
        }
    }

    private func publish(device: AVCaptureDevice) {
        deviceLock.lock()
        lockedDevice = device
        deviceLock.unlock()

        rotation.track(device)
        DispatchQueue.main.async {
            NotificationCenter.default.post(name: CaptureController.activeDeviceDidChange, object: nil)
        }
    }

    private func encoderSettings() -> VideoEncoder.Settings {
        VideoEncoder.Settings(
            codec: request.codec,
            bitrate: max(100_000, request.bitrate),
            fps: activeFormat().fps,
            keyframeInterval: request.keyframeInterval
        )
    }

    private func activeFormat() -> ActiveFormat {
        var active = ActiveFormat(
            camera: logicalCamera?.id ?? activeCamera?.id ?? request.camera,
            width: request.width,
            height: request.height,
            fps: request.fps,
            codec: request.codec,
            cameraChanged: false
        )
        if let device = activeCamera?.device {
            let dimensions = CMVideoFormatDescriptionGetDimensions(device.activeFormat.formatDescription)
            active.width = Int(dimensions.width)
            active.height = Int(dimensions.height)
            let duration = device.activeVideoMinFrameDuration
            if duration.isValid, duration.value > 0 {
                active.fps = Int((Double(duration.timescale) / Double(duration.value)).rounded())
            }
        }
        return active
    }

    // MARK: - Controls

    private func controlCapabilities() -> ControlCapabilities {
        guard let camera = activeCamera else {
            return ControlCapabilities()
        }
        let device = camera.device
        var controls = ControlCapabilities()
        let limits = zoomLimits(of: camera)
        controls.zoom = ValueRange(
            min: CaptureController.displayRounded(Double(limits.lowerBound / camera.zoomScale)),
            max: CaptureController.displayRounded(Double(limits.upperBound / camera.zoomScale))
        )
        controls.exposureBias = ValueRange(
            min: Double(device.minExposureTargetBias),
            max: Double(device.maxExposureTargetBias)
        )
        if device.isFocusModeSupported(.continuousAutoFocus) {
            controls.focus.append(.auto)
        }
        if device.isFocusModeSupported(.locked) {
            controls.focus.append(.locked)
        }
        if device.isLockingFocusWithCustomLensPositionSupported {
            controls.focus.append(.manual)
        }
        if device.isWhiteBalanceModeSupported(.continuousAutoWhiteBalance) {
            controls.whiteBalance.append(.auto)
        }
        if device.isWhiteBalanceModeSupported(.locked) {
            controls.whiteBalance.append(.locked)
        }
        controls.torch = device.hasTorch

        var modes = [StabilizationMode.off]
        if device.activeFormat.isVideoStabilizationModeSupported(.standard) {
            modes.append(.standard)
        }
        if device.activeFormat.isVideoStabilizationModeSupported(.cinematic) {
            modes.append(.cinematic)
        }
        if modes.count > 1 {
            controls.stabilization = modes
        }
        controls.faceTracking = CaptureController.faceTrackingAvailable
        // The open palm toggles face tracking, or pulls the focus in Cinematic mode.
        var gestures: [GestureKind] = []
        if CaptureController.faceTrackingAvailable || cinematicSupport != nil {
            gestures.append(.palm)
        }
        gestures.append(contentsOf: [.frame, .cross, .spread])
        controls.gestures = gestures
        controls.cinematic = cinematicSupport
        return controls
    }

    private func controlState() -> ControlValues {
        guard let camera = activeCamera else {
            return ControlValues()
        }
        let device = camera.device
        var state = ControlValues()
        state.zoom = CaptureController.displayRounded(currentZoom(of: camera))
        state.faceTracking = faceTracking
        state.gestures = gestures
        state.focus = focusMode
        if device.isLockingFocusWithCustomLensPositionSupported {
            state.focusPosition = Double(device.lensPosition)
        }
        state.exposureBias = Double(device.exposureTargetBias)
        state.whiteBalance = device.whiteBalanceMode == .locked ? .locked : .auto
        if device.hasTorch {
            state.torch = device.torchMode == .on
        }
        state.stabilization = stabilization
        if cinematicSupport != nil {
            state.cinematic = cinematicActive
            state.aperture = aperture.map { ($0 * 10).rounded() / 10 }
        }
        return state
    }

    /// Applies device controls. The device must be locked for configuration.
    private func apply(_ values: ControlValues, to device: AVCaptureDevice) {
        if let zoom = values.zoom, let camera = activeCamera, camera.device === device {
            rampZoom(to: zoom, camera: camera, rate: CaptureController.zoomRampRate)
        }

        if let target = values.cinematicFocus {
            palmFocusHeld = false
            applyCinematicFocus(target, device: device)
        }

        // Cinematic mode decides the focus itself; the lens controls wait until it is off.
        if !cinematicActive, let focus = values.focus {
            switch focus {
            case .auto:
                if device.isFocusModeSupported(.continuousAutoFocus) {
                    device.focusMode = .continuousAutoFocus
                    focusMode = .auto
                }
            case .locked:
                if device.isFocusModeSupported(.locked) {
                    device.focusMode = .locked
                    focusMode = .locked
                }
            case .manual:
                if device.isLockingFocusWithCustomLensPositionSupported {
                    let position = values.focusPosition.map { Float($0) } ?? device.lensPosition
                    device.setFocusModeLocked(lensPosition: min(max(position, 0), 1), completionHandler: nil)
                    focusMode = .manual
                }
            case .unknown:
                break
            }
        } else if !cinematicActive, let position = values.focusPosition,
            device.isLockingFocusWithCustomLensPositionSupported
        {
            device.setFocusModeLocked(lensPosition: min(max(Float(position), 0), 1), completionHandler: nil)
            focusMode = .manual
        }

        if let point = values.focusPoint {
            if cinematicActive {
                applyCinematicFocus(.track(point: point, strong: false), device: device)
            } else {
                focus(on: point, device: device)
            }
        }

        if let bias = values.exposureBias {
            let clamped = min(max(Float(bias), device.minExposureTargetBias), device.maxExposureTargetBias)
            device.setExposureTargetBias(clamped, completionHandler: nil)
        }

        if let whiteBalance = values.whiteBalance {
            switch whiteBalance {
            case .auto:
                if device.isWhiteBalanceModeSupported(.continuousAutoWhiteBalance) {
                    device.whiteBalanceMode = .continuousAutoWhiteBalance
                }
            case .locked:
                if device.isWhiteBalanceModeSupported(.locked) {
                    device.whiteBalanceMode = .locked
                }
            case .unknown:
                break
            }
        }

        if let torch = values.torch, device.hasTorch, device.isTorchAvailable {
            if !torch {
                device.torchMode = .off
            } else if device.isTorchModeSupported(.on) {
                device.torchMode = .on
            }
        }
    }

    /// Focuses and exposes once on a point of the picture the desktop shows, as a tap does.
    private func focus(on point: PicturePoint, device: AVCaptureDevice) {
        let target = self.sensorPoint(for: point)

        if device.isFocusPointOfInterestSupported, device.isFocusModeSupported(.autoFocus) {
            device.focusPointOfInterest = target
            device.focusMode = .autoFocus
            focusMode = .auto
        }
        if device.isExposurePointOfInterestSupported, device.isExposureModeSupported(.continuousAutoExposure) {
            device.exposurePointOfInterest = target
            device.exposureMode = .continuousAutoExposure
        }
    }

    /// A point of the picture the desktop shows, in the sensor's own coordinates, which focus,
    /// exposure and Cinematic mode take. The desktop's picture is rotated and mirrored by the
    /// connection; the output knows how to convert.
    private func sensorPoint(for point: PicturePoint) -> CGPoint {
        let x = min(max(point.x, 0), 1)
        let y = min(max(point.y, 0), 1)
        let size = 0.02
        let outputRect = CGRect(x: x - size / 2, y: y - size / 2, width: size, height: size)
        let sensorRect = videoOutput.metadataOutputRectConverted(fromOutputRect: outputRect)
        return CGPoint(
            x: min(max(sensorRect.midX, 0), 1),
            y: min(max(sensorRect.midY, 0), 1)
        )
    }

    // MARK: - Cinematic mode

    private func cinematicCapabilities() -> CinematicCapabilities? {
        guard #available(iOS 26.0, *) else {
            return nil
        }
        let positions = [CameraPosition.back, .front].filter { cinematicCameras[$0] != nil }
        guard let first = positions.first, let camera = cinematicCameras[first] else {
            return nil
        }
        var support = CinematicCapabilities(positions: positions)
        // Every Cinematic format offers the same aperture range in practice; take the one the
        // phone films in by default.
        if let format = CameraCatalog.cinematicFormat(for: camera.device, width: 1920, height: 1080),
            format.maxSimulatedAperture >= format.minSimulatedAperture,
            format.minSimulatedAperture > 0
        {
            support.aperture = ValueRange(
                min: Double(format.minSimulatedAperture),
                max: Double(format.maxSimulatedAperture)
            )
        }
        return support
    }

    private func setCinematic(_ on: Bool) {
        palmFocusHeld = false
        guard on != cinematicRequested || on != cinematicActive else {
            return
        }
        cinematicRequested = on
        guard isConfigured else {
            return
        }
        EngineLog.shared.info("capture", "Cinematic mode \(on ? "on" : "off")")
        reconfigureCamera()
        if on, !cinematicActive {
            // It could not start: report it off rather than keep trying on every change.
            cinematicRequested = false
        }
        let settings = encoderSettings()
        videoQueue.async {
            self.encoder.configure(settings)
            // Another camera, and perhaps another frame rate: a fresh videoConfig and keyframe.
            self.encoder.invalidate()
        }
    }

    private func setAperture(_ value: Double) {
        guard value.isFinite, value > 0 else {
            return
        }
        aperture = value
        if cinematicActive, #available(iOS 26.0, *), let input {
            applyAperture(to: input)
        }
    }

    /// Sets the input's simulated aperture to the user's choice within what the format allows,
    /// or to the format's default.
    @available(iOS 26.0, *)
    private func applyAperture(to input: AVCaptureDeviceInput) {
        let format = input.device.activeFormat
        let lowest = Double(format.minSimulatedAperture)
        let highest = Double(format.maxSimulatedAperture)
        guard lowest > 0, highest >= lowest else {
            return
        }
        let wanted = aperture ?? Double(format.defaultSimulatedAperture)
        let value = min(max(wanted, lowest), highest)
        input.simulatedAperture = Float(value)
        aperture = value
    }

    /// Moves the focus in Cinematic mode, as a tap or a long press on the Camera app's screen
    /// does. The device must be locked for configuration.
    private func applyCinematicFocus(_ target: CinematicFocus, device: AVCaptureDevice) {
        guard cinematicActive, #available(iOS 26.0, *) else {
            return
        }
        switch target {
        case .auto:
            // A weak focus on the main subject hands the choice back to the camera.
            if let subject = mainSubject() {
                device.setCinematicVideoTrackingFocus(detectedObjectID: subject.id, focusMode: .weak)
            } else {
                device.setCinematicVideoTrackingFocus(
                    at: sensorPoint(for: PicturePoint(x: 0.5, y: 0.5)),
                    focusMode: .weak
                )
            }
        case .subject(let id, let strong):
            device.setCinematicVideoTrackingFocus(detectedObjectID: id, focusMode: strong ? .strong : .weak)
        case .track(let point, let strong):
            device.setCinematicVideoTrackingFocus(at: sensorPoint(for: point), focusMode: strong ? .strong : .weak)
        case .fixed(let point):
            device.setCinematicVideoFixedFocus(at: sensorPoint(for: point), focusMode: .strong)
        case .unknown:
            break
        }
    }

    /// The person the camera would pick by itself: the largest face, else the largest head or
    /// body.
    private func mainSubject() -> CinematicSubject? {
        subjectsLock.lock()
        let subjects = seenSubjects
        subjectsLock.unlock()
        let area = { (subject: CinematicSubject) in subject.rect.width * subject.rect.height }
        for kinds in [[SubjectKind.face], [.head, .body]] {
            if let largest = subjects.filter({ kinds.contains($0.kind) }).max(by: { area($0) < area($1) }) {
                return largest
            }
        }
        return nil
    }

    private func detachMetadataOutput() {
        sceneObservation?.invalidate()
        sceneObservation = nil
        if session.outputs.contains(metadataOutput) {
            session.removeOutput(metadataOutput)
        }
    }

    /// Adds the metadata output once the session runs with Cinematic capture on, as Apple's
    /// sample does, and watches whether there is enough light.
    private func attachMetadataOutput() {
        guard #available(iOS 26.0, *), cinematicActive, let device = activeCamera?.device else {
            return
        }
        if !session.outputs.contains(metadataOutput) {
            guard session.canAddOutput(metadataOutput) else {
                EngineLog.shared.warning("capture", "the session refused the metadata output; no focus boxes")
                return
            }
            session.addOutput(metadataOutput)
        }
        metadataOutput.setMetadataObjectsDelegate(self, queue: metadataQueue)
        metadataOutput.metadataObjectTypes = metadataOutput.requiredMetadataObjectTypesForCinematicVideoCapture
        sceneObservation = device.observe(
            \.cinematicVideoCaptureSceneMonitoringStatuses,
            options: [.initial, .new]
        ) { [weak self] device, _ in
            let dark = device.cinematicVideoCaptureSceneMonitoringStatuses.contains(.notEnoughLight)
            self?.metadataQueue.async {
                self?.setLowLight(dark)
            }
        }
    }

    /// Cinematic mode is off: forget what it saw, and tell the desktop once if it was told
    /// anything.
    private func clearCinematic() {
        subjectsLock.lock()
        seenSubjects = []
        subjectsLock.unlock()
        metadataQueue.async {
            self.metadataGeneration += 1
            self.cinematicSubjects = []
            self.lowLight = false
            if self.lastCinematicReport != nil {
                self.lastCinematicReport = nil
                self.onCinematic?(CinematicState())
            }
        }
    }

    // Metadata queue.

    private func setLowLight(_ dark: Bool) {
        guard dark != lowLight else {
            return
        }
        lowLight = dark
        EngineLog.shared.info("capture", dark ? "not enough light for Cinematic mode" : "enough light again")
        publishCinematic()
    }

    /// Sends what Cinematic mode sees when it changed: at once when the focus or the light
    /// changes, and otherwise at most every `cinematicReportInterval` while subjects move.
    private func publishCinematic() {
        let state = CinematicState(subjects: cinematicSubjects, lowLight: lowLight)
        let now = ProcessInfo.processInfo.systemUptime
        if let last = lastCinematicReport {
            if last.state == state {
                return
            }
            let focused = { (state: CinematicState) in
                state.subjects.filter { $0.focus != .unfocused }.map { "\($0.id)\($0.focus.rawValue)" }
            }
            let sameShape = focused(last.state) == focused(state)
                && last.state.lowLight == state.lowLight
                && last.state.subjects.map(\.id) == state.subjects.map(\.id)
            if sameShape, now - last.time < CaptureController.cinematicReportInterval {
                return
            }
        }
        lastCinematicReport = (time: now, state: state)
        onCinematic?(state)
    }

    // MARK: - Zoom

    /// The device zoom factors the camera may use.
    private func zoomLimits(of camera: CameraDescriptor) -> ClosedRange<CGFloat> {
        let device = camera.device
        let available = device.minAvailableVideoZoomFactor...device.maxAvailableVideoZoomFactor
        var lower = available.lowerBound
        var upper = min(available.upperBound, CaptureController.maxZoom * camera.zoomScale)
        if cinematicActive, camera.id == activeCamera?.id, #available(iOS 26.0, *) {
            let format = device.activeFormat
            if format.videoMaxZoomFactorForCinematicVideo >= format.videoMinZoomFactorForCinematicVideo,
                format.videoMinZoomFactorForCinematicVideo > 0
            {
                lower = max(lower, format.videoMinZoomFactorForCinematicVideo)
                upper = min(upper, format.videoMaxZoomFactorForCinematicVideo)
            }
        }
        // Never outside what the device accepts, whatever the format says.
        lower = min(max(lower, available.lowerBound), available.upperBound)
        upper = min(max(upper, lower), available.upperBound)
        return lower...upper
    }

    /// Converts a zoom as the user sees it (1× is the wide lens) to a device factor in range.
    private func zoomFactor(forDisplay zoom: Double, camera: CameraDescriptor) -> CGFloat {
        let limits = zoomLimits(of: camera)
        let factor = CGFloat(zoom) * camera.zoomScale
        return min(max(factor, limits.lowerBound), limits.upperBound)
    }

    /// The zoom as the user sees it: where a running ramp will settle, or the current factor.
    private func currentZoom(of camera: CameraDescriptor) -> Double {
        Double((zoomTarget ?? camera.device.videoZoomFactor) / camera.zoomScale)
    }

    /// Ramps smoothly to a zoom as the user sees it. The device must be locked for configuration.
    private func rampZoom(to zoom: Double, camera: CameraDescriptor, rate: Float) {
        guard zoom.isFinite else {
            return
        }
        let device = camera.device
        let target = zoomFactor(forDisplay: zoom, camera: camera)
        device.ramp(toVideoZoomFactor: target, withRate: rate)
        zoomTarget = target
        zoomGeneration += 1
        let generation = zoomGeneration
        sessionQueue.asyncAfter(deadline: .now() + CaptureController.zoomSettleCheckInterval) { [weak self] in
            self?.checkZoomSettled(generation: generation)
        }
    }

    /// Reports the zoom once the latest ramp has finished.
    private func checkZoomSettled(generation: Int) {
        guard generation == zoomGeneration, let device = activeCamera?.device else {
            return
        }
        if device.isRampingVideoZoom {
            sessionQueue.asyncAfter(deadline: .now() + CaptureController.zoomSettleCheckInterval) { [weak self] in
                self?.checkZoomSettled(generation: generation)
            }
            return
        }
        zoomTarget = nil
        onControlsChange?(controlState())
    }

    private static func displayRounded(_ zoom: Double) -> Double {
        (zoom * 100).rounded() / 100
    }

    // MARK: - Face tracking and gestures

    private func setVision(faceTracking newFaceTracking: Bool, gestures newGestures: Bool) {
        let tracking = newFaceTracking && CaptureController.faceTrackingAvailable
        let changed = tracking != faceTracking || newGestures != gestures
        faceTracking = tracking
        gestures = newGestures
        analyzer.setOptions(VisionAnalyzer.Options(faceTracking: tracking, gestures: newGestures))
        if changed {
            EngineLog.shared.info(
                "vision",
                "face tracking \(tracking ? "on" : "off"), gestures \(newGestures ? "on" : "off")"
            )
        }
    }

    /// Acts on a gesture, then tells the desktop what happened. `point` is where the open palm
    /// was, for `palm`.
    private func handle(gesture event: HandGestureRecognizer.Event, at point: PicturePoint?) {
        // Turning gestures off ends a spread in progress; anything else that arrives afterwards
        // comes from an analysis that was already running.
        guard gestures || event == .spreadEnded else {
            return
        }
        switch event {
        case .palm:
            if cinematicActive, let point, let device = activeCamera?.device {
                do {
                    try device.lockForConfiguration()
                    if palmFocusHeld {
                        applyCinematicFocus(.auto, device: device)
                    } else {
                        applyCinematicFocus(.track(point: point, strong: true), device: device)
                    }
                    device.unlockForConfiguration()
                    palmFocusHeld.toggle()
                } catch {
                    EngineLog.shared.warning("capture", "could not lock the camera to move the focus: \(error)")
                    return
                }
                EngineLog.shared.info("vision", "gesture palm: focus \(palmFocusHeld ? "on the hand" : "back to automatic")")
                onGesture?(GestureEvent(kind: .palm, phase: .recognised))
                return
            }
            guard CaptureController.faceTrackingAvailable else {
                return
            }
            setVision(faceTracking: !faceTracking, gestures: gestures)
            EngineLog.shared.info("vision", "gesture palm: face tracking \(faceTracking ? "on" : "off")")
            onControlsChange?(controlState())
            onGesture?(GestureEvent(kind: .palm, phase: .recognised))
        case .frame:
            guard let camera = activeCamera else {
                return
            }
            let current = currentZoom(of: camera)
            let target = current * GestureTuning.zoomStep
            EngineLog.shared.info("vision", "gesture frame: zoom in from \(current) to \(target)")
            lockAndRampZoom(to: target, camera: camera, rate: CaptureController.frameZoomRampRate)
            onGesture?(GestureEvent(kind: .frame, phase: .recognised))
        case .cross:
            guard let camera = activeCamera else {
                return
            }
            let current = currentZoom(of: camera)
            // One step out, stopping at 1×. Below 1× (the ultra wide lens) it stays where it is.
            let target = current > 1.0 ? max(current / GestureTuning.zoomStep, 1.0) : current
            EngineLog.shared.info("vision", "gesture cross: zoom out from \(current) to \(target)")
            lockAndRampZoom(to: target, camera: camera, rate: CaptureController.frameZoomRampRate)
            onGesture?(GestureEvent(kind: .cross, phase: .recognised))
        case .spreadBegan:
            guard let camera = activeCamera else {
                return
            }
            let start = currentZoom(of: camera)
            spreadStartZoom = start
            EngineLog.shared.info("vision", "gesture spread began at zoom \(start)")
            onGesture?(GestureEvent(kind: .spread, phase: .began))
        case .spreadChanged(let scale):
            guard let camera = activeCamera, let start = spreadStartZoom else {
                return
            }
            let target = start * pow(scale, GestureTuning.spreadZoomExponent)
            lockAndRampZoom(to: target, camera: camera, rate: CaptureController.spreadZoomRampRate)
        case .spreadEnded:
            guard spreadStartZoom != nil else {
                return
            }
            spreadStartZoom = nil
            EngineLog.shared.info("vision", "gesture spread ended")
            onGesture?(GestureEvent(kind: .spread, phase: .ended))
        }
    }

    private func lockAndRampZoom(to zoom: Double, camera: CameraDescriptor, rate: Float) {
        do {
            try camera.device.lockForConfiguration()
            rampZoom(to: zoom, camera: camera, rate: rate)
            camera.device.unlockForConfiguration()
        } catch {
            EngineLog.shared.warning("capture", "could not lock the camera to zoom: \(error)")
        }
    }

    private static func captureMode(for mode: StabilizationMode) -> AVCaptureVideoStabilizationMode {
        switch mode {
        case .standard:
            return .standard
        case .cinematic:
            return .cinematic
        case .off, .unknown:
            return .off
        }
    }

    // MARK: - Notifications

    @objc private func sessionWasInterrupted(_ notification: Notification) {
        let reason = notification.userInfo?[AVCaptureSessionInterruptionReasonKey] ?? "unknown"
        EngineLog.shared.warning("capture", "the camera was interrupted (reason \(reason))")
        onInterruption?(true)
    }

    @objc private func sessionInterruptionEnded(_ notification: Notification) {
        EngineLog.shared.info("capture", "the camera interruption ended")
        onInterruption?(false)
        requestKeyframe()
    }

    @objc private func sessionRuntimeError(_ notification: Notification) {
        let error = notification.userInfo?[AVCaptureSessionErrorKey] as? AVError
        EngineLog.shared.error("capture", "session runtime error: \(String(describing: error))")
        guard error?.code == .mediaServicesWereReset else {
            return
        }
        sessionQueue.async {
            if self.shouldRun, !self.session.isRunning {
                self.session.startRunning()
            }
        }
    }

    /// After a one-off focus on a point, return to continuous focus once the scene changes, the
    /// way the system camera does.
    @objc private func subjectAreaDidChange(_ notification: Notification) {
        sessionQueue.async {
            guard
                !self.cinematicActive,
                let device = self.activeCamera?.device,
                (notification.object as? AVCaptureDevice) === device,
                self.focusMode == .auto,
                device.focusMode != .continuousAutoFocus,
                device.isFocusModeSupported(.continuousAutoFocus)
            else {
                return
            }
            do {
                try device.lockForConfiguration()
                if device.isFocusPointOfInterestSupported {
                    device.focusPointOfInterest = CGPoint(x: 0.5, y: 0.5)
                }
                device.focusMode = .continuousAutoFocus
                device.unlockForConfiguration()
            } catch {
                EngineLog.shared.debug("capture", "could not return to continuous focus: \(error)")
            }
        }
    }
}

extension CaptureController: AVCaptureMetadataOutputObjectsDelegate {
    /// What Cinematic mode sees, on the metadata queue.
    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard #available(iOS 26.0, *) else {
            return
        }
        let subjects = metadataObjects.compactMap { cinematicSubject(of: $0) }
        subjectsLock.lock()
        seenSubjects = subjects
        subjectsLock.unlock()
        cinematicSubjects = subjects
        publishCinematic()

        // The output may fall silent when nothing is in view; then nothing is.
        metadataGeneration += 1
        let generation = metadataGeneration
        metadataQueue.asyncAfter(deadline: .now() + CaptureController.cinematicQuietInterval) { [weak self] in
            guard let self, self.metadataGeneration == generation, !self.cinematicSubjects.isEmpty else {
                return
            }
            self.subjectsLock.lock()
            self.seenSubjects = []
            self.subjectsLock.unlock()
            self.cinematicSubjects = []
            self.publishCinematic()
        }
    }

    @available(iOS 26.0, *)
    private func cinematicSubject(of object: AVMetadataObject) -> CinematicSubject? {
        let type = object.type.rawValue.lowercased()
        // Opaque metadata for editing recordings, not something in the picture.
        guard !type.contains("cinematic") else {
            return nil
        }
        let kind = CaptureController.subjectKind(type: type, fixed: object.isFixedFocus)
        let bounds = videoOutput.outputRectConverted(fromMetadataOutputRect: object.bounds)
        var x = min(max(Double(bounds.minX), 0), 1)
        var y = min(max(Double(bounds.minY), 0), 1)
        var width = min(max(Double(bounds.width), 0), 1 - x)
        var height = min(max(Double(bounds.height), 0), 1 - y)
        if width <= 0 || height <= 0 {
            guard kind == .fixed else {
                return nil
            }
            // A fixed focus may be a bare point: give it a small square to show.
            let size = 0.06
            x = min(max(x - size / 2, 0), 1 - size)
            y = min(max(y - size / 2, 0), 1 - size)
            width = size
            height = size
        }
        let round = { (value: Double) in (value * 10_000).rounded() / 10_000 }
        return CinematicSubject(
            id: object.objectID,
            kind: kind,
            rect: PictureRect(x: round(x), y: round(y), width: round(width), height: round(height)),
            focus: CaptureController.subjectFocus(object.cinematicVideoFocusMode)
        )
    }

    private static func subjectKind(type: String, fixed: Bool) -> SubjectKind {
        if fixed {
            return .fixed
        }
        if type.contains("face") {
            return .face
        }
        if type.contains("hand") {
            return .hand
        }
        if type.contains("cat") || type.contains("dog") || type.contains("animal") {
            return .animal
        }
        if type.contains("head") {
            return .head
        }
        if type.contains("body") {
            return .body
        }
        return .object
    }

    // The SDK has given this property both as the enum and as a raw Int32; one of these two fits.
    @available(iOS 26.0, *)
    private static func subjectFocus(_ mode: AVCaptureDevice.CinematicVideoFocusMode) -> SubjectFocus {
        switch mode {
        case .strong:
            return .strong
        case .weak:
            return .weak
        default:
            return .unfocused
        }
    }

    @available(iOS 26.0, *)
    private static func subjectFocus(_ raw: Int32) -> SubjectFocus {
        // CinematicVideoFocusMode: none = 0, strong = 1, weak = 2.
        switch raw {
        case 1:
            return .strong
        case 2:
            return .weak
        default:
            return .unfocused
        }
    }
}

extension CaptureController: AVCaptureVideoDataOutputSampleBufferDelegate {
    func captureOutput(
        _ output: AVCaptureOutput,
        didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        guard let imageBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else {
            return
        }
        if isEncoding {
            encoder.encode(
                imageBuffer,
                presentationTime: CMSampleBufferGetPresentationTimeStamp(sampleBuffer),
                duration: CMSampleBufferGetDuration(sampleBuffer)
            )
        }
        // After the encoder, and it only queues the frame: analysis never delays encoding.
        analyzer.submit(imageBuffer)
    }
}
