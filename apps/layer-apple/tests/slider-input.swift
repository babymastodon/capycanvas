import Foundation

#if os(Linux)
func autoreleasepool<T>(invoking work: () throws -> T) rethrows -> T { try work() }
#endif

private final class Calls: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [Int] = []
    func record(_ value: Int) { lock.lock(); defer { lock.unlock() }; values.append(value) }
    func check(_ expected: [Int]) { lock.lock(); defer { lock.unlock() }; precondition(values == expected, "Owner order: \(values)") }
}

@main enum SliderInputChecks {
    static func main() {
        let owner = RenderOwnerQueue(), input = LatestOwnerInput<Int>(), calls = Calls()
        let entered = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0), done = DispatchSemaphore(value: 0)
        owner.async { entered.signal(); precondition(release.wait(timeout: .now() + 5) == .success) }
        precondition(entered.wait(timeout: .now() + 5) == .success)
        for value in 1...512 { input.submit("context:size", value, owner: owner) { calls.record($0) } }
        input.submit("context:strength", 600, owner: owner) { calls.record($0) }
        owner.async { calls.record(700); done.signal() }
        release.signal()
        precondition(done.wait(timeout: .now() + 5) == .success)
        calls.check([512, 600, 700])
        input.submit("context:size", 800, owner: owner) { value in
            calls.record(value)
            input.submit("context:size", 900, owner: owner) { calls.record($0); done.signal() }
        }
        precondition(done.wait(timeout: .now() + 5) == .success)
        calls.check([512, 600, 700, 800, 900])
        print("PASS slider input: latest value per setting, one owner hand-off, ordered release and subsequent input")
    }
}
