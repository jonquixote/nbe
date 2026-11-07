import Foundation

/// A payload value's shape (the export's `TypeDesc`,
/// `packages/control-plane/src/command-catalog.ts`).
public indirect enum TypeDesc: Decodable, Equatable, Sendable {
    case string(enumValues: [String]?, literal: String?)
    case number(integer: Bool, min: Double?, max: Double?)
    case boolean
    case array(items: TypeDesc)
    case object(keys: [String: KeyDesc], strict: Bool)
    case record(values: TypeDesc)
    case union(options: [TypeDesc])
    case unknown

    enum K: String, CodingKey { case type, `enum`, literal, integer, min, max, items, keys, strict, values, options }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .type) {
        case "string":
            self = .string(enumValues: try c.decodeIfPresent([String].self, forKey: .enum),
                           literal: try c.decodeIfPresent(String.self, forKey: .literal))
        case "number":
            self = .number(integer: try c.decode(Bool.self, forKey: .integer),
                           min: try c.decodeIfPresent(Double.self, forKey: .min),
                           max: try c.decodeIfPresent(Double.self, forKey: .max))
        case "boolean": self = .boolean
        case "array": self = .array(items: try c.decode(TypeDesc.self, forKey: .items))
        case "object":
            self = .object(keys: try c.decode([String: KeyDesc].self, forKey: .keys),
                           strict: try c.decode(Bool.self, forKey: .strict))
        case "record": self = .record(values: try c.decode(TypeDesc.self, forKey: .values))
        case "union": self = .union(options: try c.decode([TypeDesc].self, forKey: .options))
        default: self = .unknown
        }
    }
}

public struct KeyDesc: Decodable, Equatable, Sendable {
    public let required: Bool
    public let hasDefault: Bool
    public let nullable: Bool
    public let value: TypeDesc
}

public struct CommandDesc: Decodable, Equatable, Sendable {
    public let name: String
    public let roles: [String]
    public let payload: TypeDesc

    public func allows(_ role: Role) -> Bool { roles.contains(role.rawValue) }
}

/// The §16 catalog the control plane exports: what the shell may send, and
/// who may send it. The shell checks every command against it before it goes
/// on the wire, and the parity tests check every binding against it.
public struct CommandCatalog: Decodable, Sendable {
    public let v: String
    public let commands: [CommandDesc]

    public subscript(name: String) -> CommandDesc? { commands.first { $0.name == name } }

    /// The catalog this build carries (`Resources/commands.v0.4.json`).
    public static func bundled() throws -> CommandCatalog {
        guard let url = Bundle.module.url(forResource: "commands.v0.4", withExtension: "json") else {
            throw CatalogError.missing
        }
        return try JSONDecoder().decode(CommandCatalog.self, from: Data(contentsOf: url))
    }

    /// Why `command` with `payload` is not one the control plane accepts from
    /// `role`, or `nil` when it is (as far as the catalog says: names, keys,
    /// types, enums and integer bounds; preconditions are the server's).
    public func problems(command: String, payload: [String: JSONValue], role: Role? = nil) -> [String] {
        guard let desc = self[command] else { return ["\(command): not a §16 command"] }
        var out: [String] = []
        if let role, !desc.allows(role) {
            out.append("\(command): the §16.0 matrix does not let \(role.rawValue) issue it (\(desc.roles.joined(separator: ", ")))")
        }
        out += Self.check(.object(payload), against: desc.payload, at: command)
        return out
    }

    static func check(_ value: JSONValue, against type: TypeDesc, at path: String) -> [String] {
        switch (type, value) {
        case (.unknown, _): return []
        case (.string(let values, let literal), .string(let s)):
            if let values, !values.contains(s) { return ["\(path): \"\(s)\" is not one of \(values)"] }
            if let literal, literal != s { return ["\(path): must be \"\(literal)\""] }
            return []
        case (.number(let integer, let min, let max), .number(let n)):
            var out: [String] = []
            if integer && n.rounded() != n { out.append("\(path): \(n) is not an integer") }
            if let min, n < min { out.append("\(path): \(n) < \(min)") }
            if let max, n > max { out.append("\(path): \(n) > \(max)") }
            return out
        case (.boolean, .bool): return []
        case (.array(let items), .array(let a)):
            return a.enumerated().flatMap { check($1, against: items, at: "\(path)[\($0)]") }
        case (.record(let values), .object(let o)):
            return o.flatMap { check($1, against: values, at: "\(path).\($0)") }
        case (.union(let options), _):
            return options.contains { check(value, against: $0, at: path).isEmpty }
                ? [] : ["\(path): matches none of its \(options.count) shapes"]
        case (.object(let keys, let strict), .object(let o)):
            var out: [String] = []
            for (k, v) in o.sorted(by: { $0.key < $1.key }) {
                guard let kd = keys[k] else {
                    if strict { out.append("\(path).\(k): not a key the command takes") }
                    continue
                }
                if v.isNull && kd.nullable { continue }
                out += check(v, against: kd.value, at: "\(path).\(k)")
            }
            for (k, kd) in keys.sorted(by: { $0.key < $1.key }) where kd.required && o[k] == nil {
                out.append("\(path).\(k): required, and missing")
            }
            return out
        default:
            return ["\(path): \(value) is not a \(type)"]
        }
    }
}

public enum CatalogError: Error { case missing }
