import Foundation
import Security

/// A computer this phone has been paired with.
struct PairedHost {
    let id: DeviceId
    let name: String
    let pairedAt: Date?
}

/// Keeps the camera's device id and the pairing keys in the Keychain.
///
/// Every item is a generic password. The device id lives under one service; each paired host is
/// one item under another, with the host id as the account, the host's name as the label and the
/// 32-byte pairing key as the secret. Items stay on this device and are readable after the first
/// unlock, so reconnecting works while the screen is dimmed.
final class PairingStore {
    private let deviceService = "app.paircam.device"
    private let deviceAccount = "camera-id"
    private let hostsService = "app.paircam.paired-hosts"

    /// The camera's permanent id, created on first use.
    func deviceId() -> DeviceId {
        if let data = read(service: deviceService, account: deviceAccount), let id = DeviceId(bytes: data) {
            return id
        }
        let id = DeviceId.random()
        if !write(service: deviceService, account: deviceAccount, label: "PairCam camera id", data: id.bytes) {
            EngineLog.shared.error("store", "could not save the camera id; pairings will not survive a restart")
        }
        return id
    }

    func pairingKey(for host: DeviceId) -> Data? {
        guard let key = read(service: hostsService, account: host.hex), key.count == 32 else {
            return nil
        }
        return key
    }

    @discardableResult
    func save(host: DeviceId, name: String, key: Data) -> Bool {
        write(service: hostsService, account: host.hex, label: name, data: key)
    }

    /// Updates the name shown for a paired host without touching its key.
    func rename(host: DeviceId, to name: String) {
        let query = baseQuery(service: hostsService, account: host.hex)
        let attributes: [String: Any] = [kSecAttrLabel as String: name]
        _ = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    }

    func hosts() -> [PairedHost] {
        var query = baseQuery(service: hostsService, account: nil)
        query[kSecReturnAttributes as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitAll

        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let items = result as? [[String: Any]] else {
            return []
        }
        let hosts = items.compactMap { item -> PairedHost? in
            guard
                let account = item[kSecAttrAccount as String] as? String,
                let id = DeviceId(hex: account)
            else {
                return nil
            }
            return PairedHost(
                id: id,
                name: item[kSecAttrLabel as String] as? String ?? "",
                pairedAt: item[kSecAttrCreationDate as String] as? Date
            )
        }
        return hosts.sorted { ($0.pairedAt ?? .distantPast) > ($1.pairedAt ?? .distantPast) }
    }

    func remove(host: DeviceId) {
        _ = SecItemDelete(baseQuery(service: hostsService, account: host.hex) as CFDictionary)
    }

    // MARK: - Keychain

    private func baseQuery(service: String, account: String?) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
        ]
        if let account {
            query[kSecAttrAccount as String] = account
        }
        return query
    }

    private func read(service: String, account: String) -> Data? {
        var query = baseQuery(service: service, account: account)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess else {
            return nil
        }
        return result as? Data
    }

    private func write(service: String, account: String, label: String, data: Data) -> Bool {
        let query = baseQuery(service: service, account: account)
        _ = SecItemDelete(query as CFDictionary)

        var item = query
        item[kSecAttrLabel as String] = label
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }
}
