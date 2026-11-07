import Foundation

/// One entry of the operator-signal lane (SPEC §10.1.2; `alerts.ts`).
public struct Alert: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let severity: String
    public let reason: String
    /// Unix ms when this episode raised.
    public let since: Double
    public let auditEvent: String?
}

/// `audioDevice` (SPEC §10.1, v0.4.8 row 15).
public struct AudioDevice: Decodable, Equatable, Sendable {
    public let state: String
    public let name: String?
    public let rateHz: Double?
    public let reason: String?
}

/// The §10.1 telemetry tick. Every field is optional: the shell shows what
/// arrives and says what did not, and a field added to the tick never breaks
/// it (unknown keys are ignored).
public struct Telemetry: Decodable, Equatable, Sendable {
    public var ts: Double?
    public var showState: String?
    public var masterClockFrame: Int?
    public var viewItem: String?
    public var previewItem: String?
    public var droppedFramesTotal: Int?
    public var renderGpuTimeMs: Double?
    public var decodeSessions: Int?
    public var streamState: String?
    public var streamBufferMs: Double?
    public var streamTransportState: String?
    public var recordState: String?
    public var recordSpaceMib: Double?
    public var masterClockDriftMs: Double?
    public var fallbackActive: Bool?
    public var qualityProfile: String?
    public var degradationRung: Int?
    public var watchdogTripsTotal: Int?
    public var watchdogClearsTotal: Int?
    public var readAheadMissesTotal: Int?
    public var automationHold: Bool?
    public var audioUnderrunsTotal: Int?
    public var audioDriftMs: Double?
    /// Per-bus peak, dBFS; a bus with no signal may read null.
    public var busPeakDbfs: [String: Double?]?
    public var audioDevice: AudioDevice?
    public var recordTapPath: String?
    public var alerts: [Alert]?

    public init() {}
}

/// The control plane's status snapshot (`system.status`'s `data`, and the
/// keys a `stateChange` carries; `state.ts`'s `statusSnapshot`).
public struct StatusSnapshot: Decodable, Equatable, Sendable {
    public struct Package: Decodable, Equatable, Sendable { public let path: String }

    public var showState: String?
    public var package: Package?
    public var preflightPassed: Bool?
    public var masterClockState: String?
    public var stateVersion: Int?
    public var viewItem: String?
    public var previewItem: String?
    public var visibleOverlays: [String]?
    public var fallbackActive: Bool?
    public var recordState: String?
    public var streamState: String?
    public var automationHold: Bool?
    public var qualityProfile: String?
    public var lastError: String?
    public var alerts: [Alert]?

    public init() {}

    /// Fold a `stateChange` into this snapshot: each changed key's value
    /// replaces the one held (keys this type does not read are ignored).
    public mutating func apply(_ change: StateChange) {
        guard let merged = try? JSONValue.object(currentObject().merging(change.state) { _, new in new })
            .decode(StatusSnapshot.self)
        else { return }
        self = merged
        self.stateVersion = change.stateVersion
    }

    private func currentObject() -> [String: JSONValue] {
        guard let data = try? JSONEncoder().encode(EncodableSnapshot(self)),
              let obj = try? JSONDecoder().decode([String: JSONValue].self, from: data)
        else { return [:] }
        return obj
    }
}

/// The snapshot written back out, for `apply`'s merge.
private struct EncodableSnapshot: Encodable {
    let s: StatusSnapshot
    init(_ s: StatusSnapshot) { self.s = s }
    enum K: String, CodingKey {
        case showState, package, preflightPassed, masterClockState, stateVersion, viewItem, previewItem,
             visibleOverlays, fallbackActive, recordState, streamState, automationHold, qualityProfile, lastError, alerts
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        try c.encodeIfPresent(s.showState, forKey: .showState)
        if let p = s.package { try c.encode(["path": p.path], forKey: .package) }
        try c.encodeIfPresent(s.preflightPassed, forKey: .preflightPassed)
        try c.encodeIfPresent(s.masterClockState, forKey: .masterClockState)
        try c.encodeIfPresent(s.stateVersion, forKey: .stateVersion)
        try c.encodeIfPresent(s.viewItem, forKey: .viewItem)
        try c.encodeIfPresent(s.previewItem, forKey: .previewItem)
        try c.encodeIfPresent(s.visibleOverlays, forKey: .visibleOverlays)
        try c.encodeIfPresent(s.fallbackActive, forKey: .fallbackActive)
        try c.encodeIfPresent(s.recordState, forKey: .recordState)
        try c.encodeIfPresent(s.streamState, forKey: .streamState)
        try c.encodeIfPresent(s.automationHold, forKey: .automationHold)
        try c.encodeIfPresent(s.qualityProfile, forKey: .qualityProfile)
        try c.encodeIfPresent(s.lastError, forKey: .lastError)
        try c.encodeIfPresent(s.alerts, forKey: .alerts)
    }
}
