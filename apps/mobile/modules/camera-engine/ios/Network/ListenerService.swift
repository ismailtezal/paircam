import Foundation
import Network

/// Listens for the desktop app and advertises the camera with Bonjour.
///
/// The host always connects to the camera (see "Roles and transport" in `docs/PROTOCOL.md`), so
/// this is the phone's only network entry point. All callbacks arrive on the queue given to
/// `init`.
final class ListenerService {
    enum Event {
        /// The listener is accepting connections on this port.
        case ready(port: UInt16)
        /// Bonjour accepted the advertisement, which also means local network access is allowed.
        case advertised
        /// iOS refused local network access for this app.
        case localNetworkDenied
        case failed(String)
    }

    /// `kDNSServiceErr_PolicyDenied`: what Bonjour reports when the user declined the Local
    /// Network permission.
    private static let policyDeniedError: Int32 = -65570

    var onEvent: ((Event) -> Void)?
    var onConnection: ((NWConnection) -> Void)?

    private let queue: DispatchQueue
    private var listener: NWListener?
    private var name = ""
    private var cameraId = ""
    private var triedFallbackPort = false

    init(queue: DispatchQueue) {
        self.queue = queue
    }

    /// Starts listening on the default port, falling back to any free port if it is taken.
    func start(name: String, cameraId: DeviceId) {
        stop()
        self.name = name
        self.cameraId = cameraId.hex
        triedFallbackPort = false
        self.open(port: NWEndpoint.Port(rawValue: ProtocolConstants.defaultPort))
    }

    func stop() {
        guard let listener else {
            return
        }
        listener.stateUpdateHandler = nil
        listener.newConnectionHandler = nil
        listener.serviceRegistrationUpdateHandler = nil
        listener.cancel()
        self.listener = nil
    }

    private func open(port: NWEndpoint.Port?) {
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        let parameters = NWParameters(tls: nil, tcp: tcp)
        parameters.allowLocalEndpointReuse = true

        let listener: NWListener
        do {
            if let port {
                listener = try NWListener(using: parameters, on: port)
            } else {
                listener = try NWListener(using: parameters)
            }
        } catch {
            handleFailure(String(describing: error), usedPreferredPort: port != nil)
            return
        }

        var txt = NWTXTRecord()
        txt["v"] = String(ProtocolConstants.version)
        txt["id"] = cameraId
        txt["name"] = name
        listener.service = NWListener.Service(
            name: name,
            type: ProtocolConstants.serviceType,
            txtRecord: txt
        )

        listener.stateUpdateHandler = { [weak self, weak listener] state in
            guard let self, let listener, self.listener === listener else {
                return
            }
            switch state {
            case .ready:
                self.onEvent?(.ready(port: listener.port?.rawValue ?? 0))
            case .waiting(let error):
                if ListenerService.isPolicyDenied(error) {
                    self.onEvent?(.localNetworkDenied)
                } else {
                    EngineLog.shared.warning("listener", "waiting: \(error)")
                }
            case .failed(let error):
                if ListenerService.isPolicyDenied(error) {
                    self.onEvent?(.localNetworkDenied)
                    return
                }
                self.stop()
                self.handleFailure(String(describing: error), usedPreferredPort: port != nil)
            default:
                break
            }
        }
        listener.serviceRegistrationUpdateHandler = { [weak self, weak listener] change in
            guard let self, let listener, self.listener === listener else {
                return
            }
            if case .add = change {
                self.onEvent?(.advertised)
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            self?.onConnection?(connection)
        }

        self.listener = listener
        listener.start(queue: queue)
    }

    private func handleFailure(_ description: String, usedPreferredPort: Bool) {
        if usedPreferredPort, !triedFallbackPort {
            triedFallbackPort = true
            EngineLog.shared.warning(
                "listener",
                "port \(ProtocolConstants.defaultPort) is unavailable (\(description)); using any free port"
            )
            self.open(port: nil)
            return
        }
        onEvent?(.failed(description))
    }

    private static func isPolicyDenied(_ error: NWError) -> Bool {
        if case .dns(let code) = error {
            return Int32(code) == policyDeniedError
        }
        return false
    }
}
