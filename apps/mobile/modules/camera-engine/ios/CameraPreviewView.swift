import AVFoundation
@preconcurrency import ExpoModulesCore
import UIKit

/// The live camera picture. React Native only decides where it goes; the pixels come straight
/// from the capture session the engine owns.
public final class CameraPreviewView: ExpoView {
    // Typed as AnyObject because AVCaptureDevice.RotationCoordinator needs iOS 17.
    private var rotationCoordinator: AnyObject?
    private var rotationObservation: NSKeyValueObservation?
    private var deviceObserver: NSObjectProtocol?
    private var isActive = true

    public override class var layerClass: AnyClass {
        AVCaptureVideoPreviewLayer.self
    }

    private var previewLayer: AVCaptureVideoPreviewLayer {
        // swiftlint:disable:next force_cast
        layer as! AVCaptureVideoPreviewLayer
    }

    public required init(appContext: AppContext? = nil) {
        super.init(appContext: appContext)

        backgroundColor = .black
        clipsToBounds = true
        previewLayer.videoGravity = .resizeAspectFill
        previewLayer.session = CameraEngine.shared.capture.session

        deviceObserver = NotificationCenter.default.addObserver(
            forName: CaptureController.activeDeviceDidChange,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.followActiveDevice()
        }
        followActiveDevice()
    }

    deinit {
        rotationObservation?.invalidate()
        if let deviceObserver {
            NotificationCenter.default.removeObserver(deviceObserver)
        }
    }

    /// Pauses drawing while the screen is dimmed. Capture and streaming continue.
    func setActive(_ active: Bool) {
        isActive = active
        previewLayer.connection?.isEnabled = active
    }

    public override func layoutSubviews() {
        super.layoutSubviews()
        if #unavailable(iOS 17.0) {
            applyLegacyOrientation()
        }
    }

    private func followActiveDevice() {
        previewLayer.connection?.isEnabled = isActive

        if #available(iOS 17.0, *) {
            rotationObservation?.invalidate()
            rotationObservation = nil
            guard let device = CameraEngine.shared.capture.currentDevice else {
                rotationCoordinator = nil
                return
            }
            let coordinator = AVCaptureDevice.RotationCoordinator(device: device, previewLayer: previewLayer)
            rotationCoordinator = coordinator
            applyRotation(from: coordinator)
            rotationObservation = coordinator.observe(
                \.videoRotationAngleForHorizonLevelPreview,
                options: [.new]
            ) { [weak self] coordinator, _ in
                DispatchQueue.main.async {
                    self?.applyRotation(from: coordinator)
                }
            }
        } else {
            applyLegacyOrientation()
        }
    }

    @available(iOS 17.0, *)
    private func applyRotation(from coordinator: AVCaptureDevice.RotationCoordinator) {
        guard let connection = previewLayer.connection else {
            return
        }
        let angle = coordinator.videoRotationAngleForHorizonLevelPreview
        if connection.isVideoRotationAngleSupported(angle) {
            connection.videoRotationAngle = angle
        }
    }

    /// iOS 16: the preview follows the interface orientation.
    private func applyLegacyOrientation() {
        guard
            let connection = previewLayer.connection,
            connection.isVideoOrientationSupported,
            let interfaceOrientation = window?.windowScene?.interfaceOrientation
        else {
            return
        }
        switch interfaceOrientation {
        case .portrait:
            connection.videoOrientation = .portrait
        case .portraitUpsideDown:
            connection.videoOrientation = .portraitUpsideDown
        case .landscapeLeft:
            connection.videoOrientation = .landscapeLeft
        case .landscapeRight:
            connection.videoOrientation = .landscapeRight
        default:
            break
        }
    }
}
