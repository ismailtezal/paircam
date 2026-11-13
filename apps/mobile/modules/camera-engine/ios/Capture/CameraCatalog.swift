import AVFoundation
import CoreMedia

/// One camera the phone can stream from: a single lens, or a multi-lens virtual camera that
/// switches lenses by itself as it zooms.
struct CameraDescriptor {
    let id: String
    let device: AVCaptureDevice
    let position: CameraPosition
    let lens: CameraLens
    /// The device zoom factor shown as 1×. For a virtual camera that includes the ultra wide
    /// lens the device's 1 is the ultra wide, so the wide lens (1× to the user) sits at the first
    /// switch-over factor and the ultra wide shows as 0.5×. Every other camera uses 1.
    let zoomScale: CGFloat
}

/// Finds the phone's cameras and describes what each can do.
enum CameraCatalog {
    /// The pixel format captured and encoded: 8-bit 4:2:0, video range.
    static let pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange

    /// The camera used until the desktop asks for another one.
    static let defaultCameraId = "back-wide"

    private static let maxWidth = 3840
    private static let maxHeight = 2160
    private static let minWidth = 640
    private static let maxReportedFps = 60
    private static let minReportedFps = 15

    /// The virtual multi-lens cameras, best first. Only the best one on each side is offered.
    private static let virtualDeviceTypes: [AVCaptureDevice.DeviceType] = [
        .builtInTripleCamera,
        .builtInDualWideCamera,
        .builtInDualCamera,
    ]

    /// The cameras, in the order the desktop should list them: each single lens, then the best
    /// multi-lens virtual camera on each side, which crosses lenses seamlessly as it zooms.
    static func discover() -> [CameraDescriptor] {
        let singleLensTypes: [AVCaptureDevice.DeviceType] = [
            .builtInWideAngleCamera,
            .builtInUltraWideCamera,
            .builtInTelephotoCamera,
        ]
        let session = AVCaptureDevice.DiscoverySession(
            deviceTypes: singleLensTypes + virtualDeviceTypes,
            mediaType: .video,
            position: .unspecified
        )

        // Best virtual device first, so the weaker ones on the same side are skipped as seen.
        let devices = session.devices.sorted { left, right in
            virtualRank(of: left) < virtualRank(of: right)
        }
        var seen = Set<String>()
        var cameras = [CameraDescriptor]()
        for device in devices {
            guard let descriptor = describe(device), !seen.contains(descriptor.id) else {
                continue
            }
            seen.insert(descriptor.id)
            cameras.append(descriptor)
        }
        return cameras.sorted { order(of: $0) < order(of: $1) }
    }

    // MARK: - Cinematic mode

    /// Cinematic mode films at this many frames a second at most.
    static let cinematicMaxFps = 30

    /// The pixel formats Cinematic mode can film in, best first: video range is what the
    /// encoder takes; full range is converted by the output.
    private static let cinematicPixelFormats: [FourCharCode] = [
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    ]

    /// The camera that films in Cinematic mode on a side of the phone: the Dual Wide camera on
    /// the back and the TrueDepth camera on the front, as Apple's Cinematic Video API allows.
    /// `nil` before iOS 26, or when the phone has none that can.
    static func cinematicCamera(for position: CameraPosition) -> CameraDescriptor? {
        guard #available(iOS 26.0, *) else {
            return nil
        }
        let deviceType: AVCaptureDevice.DeviceType
        let devicePosition: AVCaptureDevice.Position
        switch position {
        case .back:
            deviceType = .builtInDualWideCamera
            devicePosition = .back
        case .front:
            deviceType = .builtInTrueDepthCamera
            devicePosition = .front
        case .external, .unknown:
            return nil
        }
        let session = AVCaptureDevice.DiscoverySession(
            deviceTypes: [deviceType],
            mediaType: .video,
            position: devicePosition
        )
        guard let device = session.devices.first(where: { $0.formats.contains(where: isCinematic) }) else {
            return nil
        }
        return CameraDescriptor(
            id: "\(position.rawValue)-cinematic",
            device: device,
            position: position,
            lens: deviceType == .builtInDualWideCamera ? .other : .wide,
            zoomScale: wideZoomFactor(of: device)
        )
    }

    /// The Cinematic format nearest to the requested size: 1080p or 4K on current iPhones.
    @available(iOS 26.0, *)
    static func cinematicFormat(for device: AVCaptureDevice, width: Int, height: Int) -> AVCaptureDevice.Format? {
        let usable = device.formats.filter(isCinematic)
        let area = width * height
        return usable.min { left, right in
            let leftDistance = distance(of: left, fromArea: area)
            let rightDistance = distance(of: right, fromArea: area)
            if leftDistance != rightDistance {
                return leftDistance < rightDistance
            }
            let leftRank = pixelFormatRank(left)
            let rightRank = pixelFormatRank(right)
            if leftRank != rightRank {
                return leftRank < rightRank
            }
            if left.isVideoBinned != right.isVideoBinned {
                return !left.isVideoBinned
            }
            return left.videoFieldOfView > right.videoFieldOfView
        }
    }

    /// The whole frame rate nearest to the request that Cinematic mode allows with a format, at
    /// most `cinematicMaxFps`.
    @available(iOS 26.0, *)
    static func cinematicFrameRate(of format: AVCaptureDevice.Format, closestTo requested: Int) -> Int {
        let wanted = min(requested, cinematicMaxFps)
        guard let range = format.videoFrameRateRangeForCinematicVideo else {
            return min(supportedFrameRate(of: format, closestTo: wanted), cinematicMaxFps)
        }
        let lowest = max(1, Int(range.minFrameRate.rounded(.up)))
        let highest = max(lowest, Int(range.maxFrameRate.rounded(.down)))
        return min(max(wanted, lowest), highest)
    }

    @available(iOS 26.0, *)
    private static func isCinematic(_ format: AVCaptureDevice.Format) -> Bool {
        format.isCinematicVideoCaptureSupported
            && cinematicPixelFormats.contains(CMFormatDescriptionGetMediaSubType(format.formatDescription))
    }

    private static func pixelFormatRank(_ format: AVCaptureDevice.Format) -> Int {
        let subtype = CMFormatDescriptionGetMediaSubType(format.formatDescription)
        return cinematicPixelFormats.firstIndex(of: subtype) ?? cinematicPixelFormats.count
    }

    // MARK: - Formats

    /// The sizes a camera can capture, each with the highest frame rate it supports there.
    static func formats(for device: AVCaptureDevice) -> [VideoFormat] {
        var best = [String: VideoFormat]()
        for format in device.formats {
            guard CMFormatDescriptionGetMediaSubType(format.formatDescription) == pixelFormat else {
                continue
            }
            let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
            let width = Int(dimensions.width)
            let height = Int(dimensions.height)
            guard width >= minWidth, width <= maxWidth, height <= maxHeight else {
                continue
            }
            let fps = min(maxReportedFps, Int(maxFrameRate(of: format).rounded()))
            guard fps >= minReportedFps else {
                continue
            }
            let key = "\(width)x\(height)"
            if (best[key]?.maxFps ?? 0) < fps {
                best[key] = VideoFormat(width: width, height: height, maxFps: fps)
            }
        }
        return best.values.sorted { left, right in
            let leftArea = left.width * left.height
            let rightArea = right.width * right.height
            if leftArea != rightArea {
                return leftArea > rightArea
            }
            return left.width > right.width
        }
    }

    /// Picks the capture format for a requested size and frame rate.
    ///
    /// An exact size match is preferred. Among those, unbinned formats win, then the format whose
    /// top frame rate is closest to the request (high-speed formats trade quality for speed),
    /// then the widest field of view. Without an exact match the closest size is used and the
    /// stream reports the size it really has.
    static func bestFormat(
        for device: AVCaptureDevice,
        width: Int,
        height: Int,
        fps: Int
    ) -> AVCaptureDevice.Format? {
        let usable = device.formats.filter { format in
            CMFormatDescriptionGetMediaSubType(format.formatDescription) == pixelFormat
        }
        let exact = usable.filter { format in
            let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
            return Int(dimensions.width) == width && Int(dimensions.height) == height
        }

        if !exact.isEmpty {
            let fastEnough = exact.filter { maxFrameRate(of: $0) >= Double(fps) }
            let candidates = fastEnough.isEmpty ? exact : fastEnough
            return candidates.min { left, right in
                if left.isVideoBinned != right.isVideoBinned {
                    return !left.isVideoBinned
                }
                let leftRate = maxFrameRate(of: left)
                let rightRate = maxFrameRate(of: right)
                if leftRate != rightRate {
                    return fastEnough.isEmpty ? leftRate > rightRate : leftRate < rightRate
                }
                return left.videoFieldOfView > right.videoFieldOfView
            }
        }

        let requestedArea = width * height
        return usable.min { left, right in
            distance(of: left, fromArea: requestedArea) < distance(of: right, fromArea: requestedArea)
        }
    }

    /// The whole frame rate nearest to the request that the format accepts. Asking a camera for a
    /// rate outside its ranges raises an exception, so the request is always clamped first.
    static func supportedFrameRate(of format: AVCaptureDevice.Format, closestTo requested: Int) -> Int {
        let ranges = format.videoSupportedFrameRateRanges
        guard !ranges.isEmpty else {
            return max(1, requested)
        }
        let wanted = Double(requested)
        if ranges.contains(where: { $0.minFrameRate <= wanted && wanted <= $0.maxFrameRate }) {
            return max(1, requested)
        }
        let lowest = ranges.map { $0.minFrameRate }.min() ?? wanted
        let highest = ranges.map { $0.maxFrameRate }.max() ?? wanted
        if wanted > highest {
            return max(1, Int(highest.rounded(.down)))
        }
        return max(1, Int(lowest.rounded(.up)))
    }

    static func maxFrameRate(of format: AVCaptureDevice.Format) -> Double {
        format.videoSupportedFrameRateRanges.map { $0.maxFrameRate }.max() ?? 0
    }

    private static func distance(of format: AVCaptureDevice.Format, fromArea area: Int) -> Int {
        let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
        return abs(Int(dimensions.width) * Int(dimensions.height) - area)
    }

    private static func describe(_ device: AVCaptureDevice) -> CameraDescriptor? {
        let position: CameraPosition
        let positionName: String
        switch device.position {
        case .back:
            position = .back
            positionName = "back"
        case .front:
            position = .front
            positionName = "front"
        default:
            return nil
        }

        let lens: CameraLens
        let lensName: String
        var zoomScale: CGFloat = 1
        switch device.deviceType {
        case .builtInWideAngleCamera:
            lens = .wide
            lensName = "wide"
        case .builtInUltraWideCamera:
            lens = .ultraWide
            lensName = "ultrawide"
        case .builtInTelephotoCamera:
            lens = .telephoto
            lensName = "telephoto"
        case .builtInTripleCamera, .builtInDualWideCamera, .builtInDualCamera:
            // One id for whichever multi-lens camera the phone has, so the desktop can remember
            // the choice across phones.
            lens = .other
            lensName = "multi"
            zoomScale = wideZoomFactor(of: device)
        default:
            return nil
        }

        return CameraDescriptor(
            id: "\(positionName)-\(lensName)",
            device: device,
            position: position,
            lens: lens,
            zoomScale: zoomScale
        )
    }

    /// The device zoom factor at which a virtual camera switches from the ultra wide to the wide
    /// lens, or 1 if it has no ultra wide lens.
    private static func wideZoomFactor(of device: AVCaptureDevice) -> CGFloat {
        guard
            device.constituentDevices.contains(where: { $0.deviceType == .builtInUltraWideCamera }),
            let switchOver = device.virtualDeviceSwitchOverVideoZoomFactors.first
        else {
            return 1
        }
        let factor = CGFloat(truncating: switchOver)
        return factor > 1 ? factor : 1
    }

    private static func virtualRank(of device: AVCaptureDevice) -> Int {
        virtualDeviceTypes.firstIndex(of: device.deviceType) ?? virtualDeviceTypes.count
    }

    private static func order(of camera: CameraDescriptor) -> Int {
        let positionOrder = camera.position == .back ? 0 : 10
        switch camera.lens {
        case .wide:
            return positionOrder
        case .ultraWide:
            return positionOrder + 1
        case .telephoto:
            return positionOrder + 2
        case .other:
            return positionOrder + 3
        }
    }
}
