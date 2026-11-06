import Foundation

/// Any JSON value, `Sendable` so it can cross actors: tool arguments and schemas.
public enum JSONValue: Sendable, Equatable, Hashable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    /// From `JSONSerialization` output; anything else becomes null.
    public init(_ any: Any?) {
        switch any {
        case nil, is NSNull: self = .null
        // `JSONSerialization` gives booleans as NSNumber too; only these are `CFBoolean`.
        case let number as NSNumber where CFGetTypeID(number) == CFBooleanGetTypeID(): self = .bool(number.boolValue)
        case let number as NSNumber: self = .number(number.doubleValue)
        case let string as String: self = .string(string)
        case let array as [Any]: self = .array(array.map(JSONValue.init))
        case let object as [String: Any]: self = .object(object.mapValues(JSONValue.init))
        default: self = .null
        }
    }

    public init?(json: String) {
        guard let object = try? JSONSerialization.jsonObject(with: Data(json.utf8), options: [.fragmentsAllowed]) else { return nil }
        self.init(object)
    }

    /// For `JSONSerialization`.
    public var any: Any {
        switch self {
        case .null: NSNull()
        case .bool(let value): value
        case .number(let value): value.rounded() == value && abs(value) < 1e15 ? Int(value) as Any : value
        case .string(let value): value
        case .array(let values): values.map(\.any)
        case .object(let values): values.mapValues(\.any)
        }
    }

    public var json: String {
        guard let data = try? JSONSerialization.data(withJSONObject: any, options: [.fragmentsAllowed, .sortedKeys, .withoutEscapingSlashes]) else { return "null" }
        return String(decoding: data, as: UTF8.self)
    }

    public subscript(key: String) -> JSONValue? {
        if case .object(let values) = self { values[key] } else { nil }
    }

    public var string: String? { if case .string(let value) = self { value } else { nil } }
    public var double: Double? { if case .number(let value) = self { value } else { nil } }
    public var int: Int? { double.map { Int($0) } }
    public var bool: Bool? { if case .bool(let value) = self { value } else { nil } }
    public var array: [JSONValue]? { if case .array(let values) = self { values } else { nil } }
}

extension JSONValue: ExpressibleByStringLiteral, ExpressibleByBooleanLiteral, ExpressibleByIntegerLiteral,
    ExpressibleByFloatLiteral, ExpressibleByArrayLiteral, ExpressibleByDictionaryLiteral, ExpressibleByNilLiteral {
    public init(stringLiteral value: String) { self = .string(value) }
    public init(booleanLiteral value: Bool) { self = .bool(value) }
    public init(integerLiteral value: Int) { self = .number(Double(value)) }
    public init(floatLiteral value: Double) { self = .number(value) }
    public init(arrayLiteral elements: JSONValue...) { self = .array(elements) }
    public init(dictionaryLiteral elements: (String, JSONValue)...) { self = .object(Dictionary(elements) { $1 }) }
    public init(nilLiteral: ()) { self = .null }
}
