import Foundation
import UIKit

/// Facts about the phone that the engine reports: its name, model, battery and temperature.
///
/// UIKit values are read on the main thread and cached, so the engine can use them from its own
/// queue.
final class DeviceStatus {
    private let lock = NSLock()
    private var cachedName = "iPhone"
    private var cachedLevel: Float = -1
    private var cachedCharging = false
    private var observers: [NSObjectProtocol] = []

    init() {
        DispatchQueue.main.async { [weak self] in
            guard let self else {
                return
            }
            UIDevice.current.isBatteryMonitoringEnabled = true
            self.refresh()

            let center = NotificationCenter.default
            for name in [UIDevice.batteryLevelDidChangeNotification, UIDevice.batteryStateDidChangeNotification] {
                let observer = center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                    self?.refresh()
                }
                self.observers.append(observer)
            }
        }
    }

    deinit {
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    /// The name shown to the user and advertised on the network.
    var deviceName: String {
        lock.lock()
        defer { lock.unlock() }
        return cachedName
    }

    /// Battery level from 0 to 1, or 0 when the system does not report one.
    var batteryLevel: Double {
        lock.lock()
        defer { lock.unlock() }
        return cachedLevel < 0 ? 0 : Double(min(cachedLevel, 1))
    }

    var isCharging: Bool {
        lock.lock()
        defer { lock.unlock() }
        return cachedCharging
    }

    var thermal: ThermalLevel {
        switch ProcessInfo.processInfo.thermalState {
        case .nominal:
            return .nominal
        case .fair:
            return .fair
        case .serious:
            return .serious
        case .critical:
            return .critical
        @unknown default:
            return .unknown
        }
    }

    /// The hardware model, such as "iPhone16,2".
    static let model: String = {
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: &info.machine) { raw in
            String(decoding: raw.prefix { $0 != 0 }, as: UTF8.self)
        }
    }()

    static let appVersion: String = {
        Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.0.0"
    }()

    /// Must run on the main thread.
    private func refresh() {
        let device = UIDevice.current
        let name = device.name
        let level = device.batteryLevel
        let charging = device.batteryState == .charging || device.batteryState == .full

        lock.lock()
        cachedName = name.isEmpty ? "iPhone" : name
        cachedLevel = level
        cachedCharging = charging
        lock.unlock()
    }
}
