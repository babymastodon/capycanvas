import Foundation

final class LatestOwnerInput<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var pending: [String: Value] = [:]

    func submit(_ key: String, _ value: Value, owner: RenderOwnerQueue, apply: @escaping @Sendable (Value) -> Void) {
        lock.lock()
        let scheduled = pending.updateValue(value, forKey: key) != nil
        lock.unlock()
        if scheduled { return }
        owner.async { [self] in
            lock.lock()
            let value = pending.removeValue(forKey: key)
            lock.unlock()
            if let value { apply(value) }
        }
    }
}
