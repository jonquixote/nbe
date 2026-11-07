import Foundation

/// The wire version (SPEC §5.4: "0.3" through v0.4, deliberately).
public let protocolVersion = "0.3"
/// The endpoint's path (SPEC §5.3).
public let wsPath = "/nbe/v0.3"
/// The default local port (SPEC §5.3).
public let defaultPort = 8462

/// The roles a token can resolve to (SPEC §5.3). `render` is the engine's.
public enum Role: String, Codable, CaseIterable, Sendable {
    case monitor, `operator`, producer, admin, render
}

/// A client-to-server command (SPEC §5.4). Never carries `kind` (§5.4.1 rule 4).
public struct CommandEnvelope: Encodable, Equatable, Sendable {
    public let v: String
    public let id: String
    public let command: String
    public let payload: [String: JSONValue]
    public let baseStateVersion: Int?

    public init(id: String = UUID().uuidString.lowercased(), command: String, payload: [String: JSONValue],
                baseStateVersion: Int? = nil) {
        self.v = protocolVersion
        self.id = id
        self.command = command
        self.payload = payload
        self.baseStateVersion = baseStateVersion
    }

    enum CodingKeys: String, CodingKey { case v, id, command, payload, baseStateVersion }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(v, forKey: .v)
        try c.encode(id, forKey: .id)
        try c.encode(command, forKey: .command)
        try c.encode(payload, forKey: .payload)
        try c.encodeIfPresent(baseStateVersion, forKey: .baseStateVersion)
    }
}

/// A §5.4 error body.
public struct ErrorBody: Decodable, Equatable, Sendable {
    public let code: String
    public let message: String
}

/// A command's response (SPEC §5.4): matched to its command by `requestId`.
public struct CommandResponse: Decodable, Equatable, Sendable {
    public let v: String
    public let requestId: String
    public let status: String
    public let stateVersion: Int
    public let data: JSONValue?
    public let error: ErrorBody?

    public var ok: Bool { status == "ok" }
}

/// A server-push `stateChange` (SPEC §5.4.1): once per accepted command, with
/// the changed top-level keys and their new values (the full snapshot when
/// `changed` is absent).
public struct StateChange: Decodable, Equatable, Sendable {
    public let v: String
    public let stateVersion: Int
    public let changed: [String]?
    public let state: [String: JSONValue]
}

/// A server-push `telemetry` frame (SPEC §5.4.1): `data` is the §10.1 shape.
public struct TelemetryFrame: Decodable, Equatable, Sendable {
    public let v: String
    public let data: Telemetry
}

/// Everything the server sends a client, told apart as §5.4.1 says: a frame
/// with a `requestId` is a response; a server-initiated frame has a `kind`.
public enum InboundFrame: Equatable, Sendable {
    case response(CommandResponse)
    case stateChange(StateChange)
    case telemetry(TelemetryFrame)
    /// A server-initiated frame of a kind this shell does not read yet.
    case unknown(kind: String)

    public static func decode(_ data: Data) throws -> InboundFrame {
        let any = try JSONDecoder().decode(JSONValue.self, from: data)
        if any["requestId"] != nil {
            return .response(try JSONDecoder().decode(CommandResponse.self, from: data))
        }
        switch any["kind"]?.string {
        case "stateChange"?: return .stateChange(try JSONDecoder().decode(StateChange.self, from: data))
        case "telemetry"?: return .telemetry(try JSONDecoder().decode(TelemetryFrame.self, from: data))
        case let kind?: return .unknown(kind: kind)
        case nil:
            throw DecodingError.dataCorrupted(
                .init(codingPath: [], debugDescription: "a frame with neither requestId nor kind"))
        }
    }
}
