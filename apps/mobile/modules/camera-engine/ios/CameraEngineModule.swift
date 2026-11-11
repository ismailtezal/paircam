import AVFoundation
@preconcurrency import ExpoModulesCore

/// The JavaScript face of the camera engine. It forwards calls to `CameraEngine` and turns the
/// engine's callbacks into events; no frame ever passes through here.
public final class CameraEngineModule: Module {
    private static let stateEvent = "onStateChange"
    private static let environmentEvent = "onEnvironmentChange"
    private static let statsEvent = "onStats"
    private static let disconnectEvent = "onDisconnect"

    public func definition() -> ModuleDefinition {
        Name("CameraEngine")

        Events(
            CameraEngineModule.stateEvent,
            CameraEngineModule.environmentEvent,
            CameraEngineModule.statsEvent,
            CameraEngineModule.disconnectEvent
        )

        OnCreate {
            var callbacks = CameraEngine.Callbacks()
            callbacks.onStateChange = { [weak self] state in
                self?.sendEvent(CameraEngineModule.stateEvent, state.dictionary)
            }
            callbacks.onEnvironmentChange = { [weak self] environment in
                self?.sendEvent(CameraEngineModule.environmentEvent, environment.dictionary)
            }
            callbacks.onStats = { [weak self] stats in
                self?.sendEvent(CameraEngineModule.statsEvent, stats)
            }
            callbacks.onDisconnect = { [weak self] event in
                self?.sendEvent(CameraEngineModule.disconnectEvent, event)
            }
            CameraEngine.shared.callbacks = callbacks
        }

        OnDestroy {
            CameraEngine.shared.callbacks = CameraEngine.Callbacks()
        }

        Function("getState") { () -> [String: Any] in
            CameraEngine.shared.stateSnapshot.dictionary
        }

        Function("getEnvironment") { () -> [String: Any] in
            CameraEngine.shared.environmentSnapshot.dictionary
        }

        AsyncFunction("start") {
            CameraEngine.shared.start()
        }

        AsyncFunction("stop") {
            CameraEngine.shared.stop()
        }

        AsyncFunction("respondToPairing") { (accept: Bool) in
            CameraEngine.shared.respondToPairing(accept: accept)
        }

        AsyncFunction("disconnect") {
            CameraEngine.shared.disconnect()
        }

        AsyncFunction("getPairedHosts") { (promise: Promise) in
            CameraEngine.shared.pairedHosts { hosts in
                promise.resolve(hosts as Any?)
            }
        }

        AsyncFunction("forgetHost") { (id: String, promise: Promise) in
            CameraEngine.shared.forgetHost(id: id) {
                promise.resolve()
            }
        }

        AsyncFunction("requestCameraPermission") { (promise: Promise) in
            AVCaptureDevice.requestAccess(for: .video) { _ in
                CameraEngine.shared.permissionDidChange()
                promise.resolve(EngineEnvironment.cameraPermissionName())
            }
        }

        View(CameraPreviewView.self) {
            Prop("active") { (view: CameraPreviewView, active: Bool) in
                view.setActive(active)
            }
        }
    }
}
