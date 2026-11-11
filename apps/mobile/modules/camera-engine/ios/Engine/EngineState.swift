import AVFoundation
import Foundation

/// What the engine is doing, as the interface sees it.
enum EngineState: Equatable {
    /// The service is stopped.
    case idle
    /// Waiting for a computer to connect.
    case advertising
    /// A computer wants to pair; the user must compare the code and decide.
    case pairing(hostName: String, code: String)
    /// A paired computer is connected but not streaming.
    case connected(hostName: String)
    case streaming(hostName: String, width: Int, height: Int, fps: Int, codec: VideoCodec)

    var hostName: String? {
        switch self {
        case .idle, .advertising:
            return nil
        case .pairing(let hostName, _), .connected(let hostName), .streaming(let hostName, _, _, _, _):
            return hostName
        }
    }

    /// The shape the TypeScript wrapper expects (see `src/CameraEngine.types.ts`).
    var dictionary: [String: Any] {
        switch self {
        case .idle:
            return ["status": "idle"]
        case .advertising:
            return ["status": "advertising"]
        case .pairing(let hostName, let code):
            return ["status": "pairing", "hostName": hostName, "code": code]
        case .connected(let hostName):
            return ["status": "connected", "hostName": hostName]
        case .streaming(let hostName, let width, let height, let fps, let codec):
            return [
                "status": "streaming",
                "hostName": hostName,
                "width": width,
                "height": height,
                "fps": fps,
                "codec": codec.rawValue,
            ]
        }
    }
}

/// Things around the engine that the interface explains to the user.
struct EngineEnvironment: Equatable {
    enum LocalNetwork: String {
        /// The system has not answered yet (or has not been asked).
        case unknown
        case granted
        case denied
    }

    var cameraPermission = EngineEnvironment.cameraPermissionName()
    var localNetwork = LocalNetwork.unknown
    /// The phone is on Wi-Fi (or wired Ethernet), so a computer on the same network can reach it.
    var wifi = false
    /// Another app or the system has taken the camera away for the moment.
    var cameraInterrupted = false
    var deviceName = "iPhone"
    /// The TCP port the service listens on, or 0 while it is stopped.
    var port = 0
    /// The camera is finding the person for the computer to frame. Only ever on in a session.
    var faceTracking = false
    /// The camera is watching for hand gestures. Only ever on in a session.
    var gestures = false

    var dictionary: [String: Any] {
        [
            "cameraPermission": cameraPermission,
            "localNetwork": localNetwork.rawValue,
            "wifi": wifi,
            "cameraInterrupted": cameraInterrupted,
            "deviceName": deviceName,
            "port": port,
            "faceTracking": faceTracking,
            "gestures": gestures,
        ]
    }

    static func cameraPermissionName() -> String {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            return "granted"
        case .denied:
            return "denied"
        case .restricted:
            return "restricted"
        case .notDetermined:
            return "undetermined"
        @unknown default:
            return "undetermined"
        }
    }
}
