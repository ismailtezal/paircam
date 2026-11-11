import AVFoundation
import UIKit

/// The camera: one capture session shared by the on-screen preview and the encoder.
///
/// The session runs whenever the app is in the foreground so the preview is always live. Frames
/// reach the encoder only while a computer is streaming. Session and device changes happen on a
/// private session queue; frames are delivered and encoded on a private video queue.
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

    private static let maxZoom: CGFloat = 10

    let session = AVCaptureSession()

    /// Compressed frames, delivered on a VideoToolbox thread.
    var onEncodedFrame: ((VideoEncoder.Output) -> Void)?
    /// The camera was taken away (`true`) or given back (`false`). Any thread.
    var onInterruption: ((Bool) -> Void)?

    private let sessionQueue = DispatchQueue(label: "app.paircam.capture.session")
    private let videoQueue = DispatchQueue(label: "app.paircam.capture.video", qos: .userInteractive)
    private let videoOutput = AVCaptureVideoDataOutput()
    private let rotation = CaptureRotation()
    private let encoder: VideoEncoder

    // Session queue state.
    private var isConfigured = false
    private var shouldRun = false
    private var cameras: [CameraDescriptor] = []
    private var activeCamera: CameraDescriptor?
    private var input: AVCaptureDeviceInput?
    private var request = Request()
    private var focusMode = FocusMode.auto
    private var stabilization = StabilizationMode.off

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
            let previousCamera = self.activeCamera?.id
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
                self.session.beginConfiguration()
                self.selectCamera()
                self.session.commitConfiguration()
                self.configureConnection()
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

    /// Changes the controls named in `values` and reports the state of all of them.
    func applyControl(_ values: ControlValues, completion: @escaping (ControlValues) -> Void) {
        sessionQueue.async {
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

    /// Switches to the requested camera, format and frame rate. Call inside a configuration block.
    private func selectCamera() {
        let requested = cameras.first(where: { $0.id == request.camera })
        let fallback = cameras.first(where: { $0.id == CameraCatalog.defaultCameraId })
        guard let camera = requested ?? activeCamera ?? fallback ?? cameras.first else {
            return
        }

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
        guard
            let format = CameraCatalog.bestFormat(
                for: device,
                width: request.width,
                height: request.height,
                fps: request.fps
            )
        else {
            EngineLog.shared.warning("capture", "camera \(camera.id) has no usable format")
            return
        }
        do {
            try device.lockForConfiguration()
            device.activeFormat = format
            let fps = CameraCatalog.supportedFrameRate(of: format, closestTo: request.fps)
            let frameDuration = CMTime(value: 1, timescale: CMTimeScale(fps))
            device.activeVideoMinFrameDuration = frameDuration
            device.activeVideoMaxFrameDuration = frameDuration
            if focusMode == .auto, device.isFocusModeSupported(.continuousAutoFocus) {
                device.focusMode = .continuousAutoFocus
            }
            device.isSubjectAreaChangeMonitoringEnabled = true
            device.unlockForConfiguration()
        } catch {
            EngineLog.shared.warning("capture", "could not set the format on \(camera.id): \(error)")
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
            camera: activeCamera?.id ?? request.camera,
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
        guard let device = activeCamera?.device else {
            return ControlCapabilities()
        }
        var controls = ControlCapabilities()
        controls.zoom = ValueRange(
            min: Double(device.minAvailableVideoZoomFactor),
            max: Double(min(device.maxAvailableVideoZoomFactor, CaptureController.maxZoom))
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
        return controls
    }

    private func controlState() -> ControlValues {
        guard let device = activeCamera?.device else {
            return ControlValues()
        }
        var state = ControlValues()
        state.zoom = Double(device.videoZoomFactor)
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
        return state
    }

    /// Applies device controls. The device must be locked for configuration.
    private func apply(_ values: ControlValues, to device: AVCaptureDevice) {
        if let zoom = values.zoom {
            let upper = min(device.maxAvailableVideoZoomFactor, CaptureController.maxZoom)
            device.videoZoomFactor = min(max(CGFloat(zoom), device.minAvailableVideoZoomFactor), upper)
        }

        if let focus = values.focus {
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
        } else if let position = values.focusPosition, device.isLockingFocusWithCustomLensPositionSupported {
            device.setFocusModeLocked(lensPosition: min(max(Float(position), 0), 1), completionHandler: nil)
            focusMode = .manual
        }

        if let point = values.focusPoint {
            focus(on: point, device: device)
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
        let x = min(max(point.x, 0), 1)
        let y = min(max(point.y, 0), 1)
        // The desktop's picture is rotated and mirrored by the connection; the camera wants the
        // point in the sensor's own coordinates. The output knows how to convert.
        let size = 0.02
        let outputRect = CGRect(x: x - size / 2, y: y - size / 2, width: size, height: size)
        let sensorRect = videoOutput.metadataOutputRectConverted(fromOutputRect: outputRect)
        let sensorPoint = CGPoint(
            x: min(max(sensorRect.midX, 0), 1),
            y: min(max(sensorRect.midY, 0), 1)
        )

        if device.isFocusPointOfInterestSupported, device.isFocusModeSupported(.autoFocus) {
            device.focusPointOfInterest = sensorPoint
            device.focusMode = .autoFocus
            focusMode = .auto
        }
        if device.isExposurePointOfInterestSupported, device.isExposureModeSupported(.continuousAutoExposure) {
            device.exposurePointOfInterest = sensorPoint
            device.exposureMode = .continuousAutoExposure
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

extension CaptureController: AVCaptureVideoDataOutputSampleBufferDelegate {
    func captureOutput(
        _ output: AVCaptureOutput,
        didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        guard isEncoding, let imageBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else {
            return
        }
        encoder.encode(
            imageBuffer,
            presentationTime: CMSampleBufferGetPresentationTimeStamp(sampleBuffer),
            duration: CMSampleBufferGetDuration(sampleBuffer)
        )
    }
}
