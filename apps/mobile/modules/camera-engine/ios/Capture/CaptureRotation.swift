import AVFoundation
import UIKit

/// Keeps the captured picture upright however the phone is held.
///
/// The desktop does not rotate or mirror, so the capture connection does it and the encoder sees
/// frames that are already upright. On iOS 17 and later the system's rotation coordinator follows
/// gravity, which works even with the orientation lock on. iOS 16 falls back to device
/// orientation notifications, which stop updating while the orientation lock is on.
final class CaptureRotation {
    /// Called on the main thread when the upright angle may have changed.
    var onChange: (() -> Void)?

    // Typed as AnyObject because AVCaptureDevice.RotationCoordinator needs iOS 17.
    private var coordinator: AnyObject?
    private var observation: NSKeyValueObservation?
    private var orientationObserver: NSObjectProtocol?
    private let lock = NSLock()
    private var legacyOrientation = AVCaptureVideoOrientation.portrait

    deinit {
        observation?.invalidate()
        if let orientationObserver {
            NotificationCenter.default.removeObserver(orientationObserver)
        }
    }

    /// Follows the given camera. Call again whenever the active camera changes.
    func track(_ device: AVCaptureDevice) {
        DispatchQueue.main.async { [weak self] in
            guard let self else {
                return
            }
            if #available(iOS 17.0, *) {
                self.observation?.invalidate()
                let coordinator = AVCaptureDevice.RotationCoordinator(device: device, previewLayer: nil)
                self.lock.lock()
                self.coordinator = coordinator
                self.lock.unlock()
                self.observation = coordinator.observe(
                    \.videoRotationAngleForHorizonLevelCapture,
                    options: [.new]
                ) { [weak self] _, _ in
                    self?.onChange?()
                }
            } else {
                self.startLegacyTracking()
            }
            self.onChange?()
        }
    }

    /// Applies the current upright rotation to a capture connection. Returns `true` if the
    /// connection changed. Safe to call from the capture session queue.
    func apply(to connection: AVCaptureConnection) -> Bool {
        if #available(iOS 17.0, *) {
            lock.lock()
            let tracked = self.coordinator as? AVCaptureDevice.RotationCoordinator
            lock.unlock()
            guard let coordinator = tracked else {
                return false
            }
            let angle = coordinator.videoRotationAngleForHorizonLevelCapture
            guard
                connection.isVideoRotationAngleSupported(angle),
                connection.videoRotationAngle != angle
            else {
                return false
            }
            connection.videoRotationAngle = angle
            return true
        }

        lock.lock()
        let orientation = legacyOrientation
        lock.unlock()
        guard connection.isVideoOrientationSupported, connection.videoOrientation != orientation else {
            return false
        }
        connection.videoOrientation = orientation
        return true
    }

    private func startLegacyTracking() {
        guard orientationObserver == nil else {
            return
        }
        UIDevice.current.beginGeneratingDeviceOrientationNotifications()
        updateLegacyOrientation()
        orientationObserver = NotificationCenter.default.addObserver(
            forName: UIDevice.orientationDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.updateLegacyOrientation()
            self?.onChange?()
        }
    }

    private func updateLegacyOrientation() {
        // Device and capture orientations name the two landscapes the opposite way round.
        let orientation: AVCaptureVideoOrientation
        switch UIDevice.current.orientation {
        case .portrait:
            orientation = .portrait
        case .portraitUpsideDown:
            orientation = .portraitUpsideDown
        case .landscapeLeft:
            orientation = .landscapeRight
        case .landscapeRight:
            orientation = .landscapeLeft
        default:
            // Face up, face down or unknown: keep the last upright orientation.
            return
        }
        lock.lock()
        legacyOrientation = orientation
        lock.unlock()
    }
}
