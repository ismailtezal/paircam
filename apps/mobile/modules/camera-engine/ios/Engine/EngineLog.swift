import Foundation
import os

/// The engine's log. Lines go to the system log and, once a computer is connected, to the desktop
/// app as `log` messages, which is how the phone is debugged without a Mac.
///
/// Lines written while nobody is connected are kept in a small backlog and delivered when the next
/// computer connects, so start-up problems are visible too.
final class EngineLog {
    static let shared = EngineLog()

    private static let backlogLimit = 200

    private let logger = os.Logger(subsystem: "app.paircam", category: "engine")
    private let lock = NSLock()
    private var backlog: [LogEntry] = []
    private var sink: ((LogEntry) -> Void)?

    private init() {}

    func debug(_ tag: String, _ message: String) {
        write(.debug, tag, message)
    }

    func info(_ tag: String, _ message: String) {
        write(.info, tag, message)
    }

    func warning(_ tag: String, _ message: String) {
        write(.warning, tag, message)
    }

    func error(_ tag: String, _ message: String) {
        write(.error, tag, message)
    }

    /// Starts forwarding lines to `sink`, beginning with the backlog. The sink may be called from
    /// any thread and must not log.
    func attach(_ sink: @escaping (LogEntry) -> Void) {
        lock.lock()
        let pending = backlog
        backlog.removeAll()
        self.sink = sink
        lock.unlock()

        for entry in pending {
            sink(entry)
        }
    }

    func detach() {
        lock.lock()
        sink = nil
        lock.unlock()
    }

    private func write(_ level: LogLevel, _ tag: String, _ message: String) {
        switch level {
        case .debug:
            logger.debug("[\(tag, privacy: .public)] \(message, privacy: .public)")
        case .warning:
            logger.warning("[\(tag, privacy: .public)] \(message, privacy: .public)")
        case .error:
            logger.error("[\(tag, privacy: .public)] \(message, privacy: .public)")
        default:
            logger.info("[\(tag, privacy: .public)] \(message, privacy: .public)")
        }

        let entry = LogEntry(level: level, tag: tag, message: message)
        lock.lock()
        let sink = self.sink
        if sink == nil {
            backlog.append(entry)
            if backlog.count > EngineLog.backlogLimit {
                backlog.removeFirst(backlog.count - EngineLog.backlogLimit)
            }
        }
        lock.unlock()

        sink?(entry)
    }
}
