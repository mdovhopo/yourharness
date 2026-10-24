import Foundation
import SQLite3

/// Just enough SQLite: one connection, prepared statements cached by their SQL, typed
/// binds and columns. Not thread-safe: each connection is only used under a lock, which is
/// what makes it safe to hand over.
final class SQLiteConnection: @unchecked Sendable {
    struct Failure: Error, CustomStringConvertible {
        let code: Int32
        let message: String
        var description: String { "SQLite error \(code): \(message)" }
    }

    private var handle: OpaquePointer?
    private var statements: [String: OpaquePointer] = [:]

    init(url: URL, readOnly: Bool = false) throws(Failure) {
        let flags = readOnly
            ? SQLITE_OPEN_READONLY | SQLITE_OPEN_NOMUTEX
            : SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_NOMUTEX
        let path = url.isFileURL ? url.path : url.absoluteString
        let code = sqlite3_open_v2(path, &handle, flags, nil)
        guard code == SQLITE_OK else {
            let message = handle.map { String(cString: sqlite3_errmsg($0)) } ?? "can't open"
            sqlite3_close_v2(handle)
            handle = nil
            throw Failure(code: code, message: message)
        }
        // A writer holding the lock makes a reader wait a moment rather than fail.
        sqlite3_busy_timeout(handle, 5_000)
    }

    deinit {
        for statement in statements.values { sqlite3_finalize(statement) }
        sqlite3_close_v2(handle)
    }

    /// Runs one or more statements with no parameters or results.
    func execute(_ sql: String) throws(Failure) {
        var error: UnsafeMutablePointer<CChar>?
        let code = sqlite3_exec(handle, sql, nil, nil, &error)
        guard code == SQLITE_OK else {
            let message = error.map { String(cString: $0) } ?? "unknown"
            sqlite3_free(error)
            throw Failure(code: code, message: message)
        }
    }

    /// Runs one statement, binding `values` in order.
    func run(_ sql: String, _ values: [SQLiteValue] = []) throws(Failure) {
        let statement = try prepare(sql, values)
        defer { sqlite3_reset(statement) }
        let code = sqlite3_step(statement)
        guard code == SQLITE_DONE || code == SQLITE_ROW else { throw failure(code) }
    }

    /// Each result row, handed to `body` while it's current.
    func query(_ sql: String, _ values: [SQLiteValue] = [], _ body: (SQLiteRow) throws -> Void) throws {
        let statement = try prepare(sql, values)
        defer { sqlite3_reset(statement) }
        while true {
            let code = sqlite3_step(statement)
            if code == SQLITE_DONE { return }
            guard code == SQLITE_ROW else { throw failure(code) }
            try body(SQLiteRow(statement: statement))
        }
    }

    /// The first row's first column, if any.
    func scalar(_ sql: String, _ values: [SQLiteValue] = []) throws -> SQLiteRow.Value? {
        var result: SQLiteRow.Value?
        try query(sql, values) { row in if result == nil { result = row.value(0) } }
        return result
    }

    /// All or nothing: rolled back if `body` throws.
    func transaction<T>(_ body: () throws -> T) throws -> T {
        try execute("BEGIN IMMEDIATE")
        do {
            let result = try body()
            try execute("COMMIT")
            return result
        } catch {
            try? execute("ROLLBACK")
            throw error
        }
    }

    var changes: Int { Int(sqlite3_changes(handle)) }

    private func prepare(_ sql: String, _ values: [SQLiteValue]) throws(Failure) -> OpaquePointer {
        let statement: OpaquePointer
        if let cached = statements[sql] {
            statement = cached
        } else {
            var prepared: OpaquePointer?
            let code = sqlite3_prepare_v3(handle, sql, -1, UInt32(SQLITE_PREPARE_PERSISTENT), &prepared, nil)
            guard code == SQLITE_OK, let prepared else { throw failure(code) }
            statements[sql] = prepared
            statement = prepared
        }
        sqlite3_clear_bindings(statement)
        for (offset, value) in values.enumerated() {
            let index = Int32(offset + 1)
            let code: Int32
            switch value {
            case .null: code = sqlite3_bind_null(statement, index)
            case .int(let int): code = sqlite3_bind_int64(statement, index, int)
            case .double(let double): code = sqlite3_bind_double(statement, index, double)
            case .text(let text): code = sqlite3_bind_text(statement, index, text, -1, Self.transient)
            case .blob(let data):
                code = data.withUnsafeBytes { bytes in
                    // A zero-length blob still has to be a blob, not NULL.
                    sqlite3_bind_blob(statement, index, bytes.baseAddress ?? UnsafeRawPointer(bitPattern: 1), Int32(bytes.count), Self.transient)
                }
            }
            guard code == SQLITE_OK else { throw failure(code) }
        }
        return statement
    }

    private func failure(_ code: Int32) -> Failure {
        Failure(code: code, message: String(cString: sqlite3_errmsg(handle)))
    }

    private static let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
}

enum SQLiteValue {
    case null
    case int(Int64)
    case double(Double)
    case text(String)
    case blob(Data)

    static func of(_ string: String?) -> SQLiteValue { string.map(SQLiteValue.text) ?? .null }
    static func of(_ int: Int?) -> SQLiteValue { int.map { .int(Int64($0)) } ?? .null }
    static func of(_ bool: Bool?) -> SQLiteValue { bool.map { .int($0 ? 1 : 0) } ?? .null }
    static func of(_ date: Date?) -> SQLiteValue { date.map { .double($0.timeIntervalSince1970) } ?? .null }
    static func of(_ data: Data?) -> SQLiteValue { data.map(SQLiteValue.blob) ?? .null }
}

/// The current row of a query; only valid inside the query's closure.
struct SQLiteRow {
    enum Value: Equatable {
        case int(Int64)
        case double(Double)
        case text(String)
        case blob(Data)
    }

    fileprivate let statement: OpaquePointer

    func isNull(_ column: Int32) -> Bool { sqlite3_column_type(statement, column) == SQLITE_NULL }

    func int(_ column: Int32) -> Int? {
        isNull(column) ? nil : Int(sqlite3_column_int64(statement, column))
    }

    func double(_ column: Int32) -> Double? {
        isNull(column) ? nil : sqlite3_column_double(statement, column)
    }

    func text(_ column: Int32) -> String? {
        guard let pointer = sqlite3_column_text(statement, column) else { return nil }
        return String(cString: pointer)
    }

    func data(_ column: Int32) -> Data? {
        guard !isNull(column) else { return nil }
        let count = Int(sqlite3_column_bytes(statement, column))
        guard count > 0, let pointer = sqlite3_column_blob(statement, column) else { return Data() }
        return Data(bytes: pointer, count: count)
    }

    func date(_ column: Int32) -> Date? {
        double(column).map(Date.init(timeIntervalSince1970:))
    }

    func value(_ column: Int32) -> Value? {
        switch sqlite3_column_type(statement, column) {
        case SQLITE_INTEGER: .int(sqlite3_column_int64(statement, column))
        case SQLITE_FLOAT: .double(sqlite3_column_double(statement, column))
        case SQLITE_TEXT: text(column).map(Value.text)
        case SQLITE_BLOB: data(column).map(Value.blob)
        default: nil
        }
    }
}

extension SQLiteRow.Value {
    var int: Int? { if case .int(let value) = self { Int(value) } else { nil } }
}
