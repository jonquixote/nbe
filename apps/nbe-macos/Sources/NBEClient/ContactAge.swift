import Foundation

/// How fresh the shell's picture of the show is (operator failure design,
/// rule 1 and build step 2): when the newest frame from the control plane
/// was RECEIVED. The age is never taken from a tick's own `ts`: that is the
/// sender's clock, which can be ahead of this machine's (a shell on another
/// Mac), and a stamp from the future never ages.
public struct ContactAge: Sendable, Equatable {
    /// Past this with nothing received, every telemetry-fed value is stale and
    /// the NO CONTACT curtain is up: the control plane's own staleness
    /// threshold (`ENGINE_TELEMETRY_TTL_MS`, 2 s).
    public static let limit: TimeInterval = 2.0

    /// When the newest frame was received; nil while not tracking (no
    /// session yet, or the fixture).
    public private(set) var lastReceivedAt: Date?
    /// The newest tick's own stamp, kept for display only.
    public private(set) var lastTickTs: Double?

    public init() {}

    /// A frame arrived at `at` (this machine's clock). `tickTs` is the tick's
    /// own stamp, if it had one.
    public mutating func received(tickTs: Double? = nil, at: Date) {
        lastReceivedAt = at
        if let tickTs { lastTickTs = tickTs }
    }

    /// Stop tracking (the session is over and nothing is shown as live).
    public mutating func reset() {
        lastReceivedAt = nil
        lastTickTs = nil
    }

    /// Seconds since the newest frame was received; nil while not tracking.
    public func age(now: Date) -> TimeInterval? {
        lastReceivedAt.map { max(0, now.timeIntervalSince($0)) }
    }

    /// Whether the values shown are stale: tracking, and nothing received
    /// for longer than `limit`.
    public func isStale(now: Date) -> Bool {
        guard let age = age(now: now) else { return false }
        return age > Self.limit
    }
}
