import Foundation
import SQLite3

final class TestWorkspaceDatabase {
    private let handle: OpaquePointer
    init(directory: URL) throws {
        var handle: OpaquePointer?
        guard sqlite3_open(directory.appendingPathComponent("workspaces.sqlite3").path, &handle) == SQLITE_OK, let handle else {
            throw HostFailure(message: "Could not open fixture SQLite connection")
        }
        self.handle = handle
        sqlite3_busy_timeout(handle, 10_000)
    }
    deinit { sqlite3_close(handle) }
    func execute(_ sql: String) throws {
        guard sqlite3_exec(handle, sql, nil, nil, nil) == SQLITE_OK else {
            throw HostFailure(message: String(cString: sqlite3_errmsg(handle)))
        }
    }
}
