import Foundation

/// `show.getState`'s data (PROPOSAL, finding 1): the loaded show as the
/// shell draws it. Every field optional: a control plane without the
/// command answers E_UNKNOWN_COMMAND and the shell says so.
public struct ShowStateView: Decodable, Equatable, Sendable {
    public struct Item: Decodable, Equatable, Sendable, Identifiable {
        public let id: String
        public let kind: String
        public let durationFrames: Int?
        public let autoFollow: Bool?
        /// §17.1: READY, ARMED, LIVE, PLAYING, DONE, MISSING, ERROR.
        public let state: String
    }
    public struct Rundown: Decodable, Equatable, Sendable {
        public let id: String?
        public let items: [Item]
    }
    public struct Element: Decodable, Equatable, Sendable, Identifiable {
        public let id: String
        public let kind: String
    }

    public var stateVersion: Int?
    public var showState: String?
    public var rundown: Rundown?
    public var elements: [Element]?
    public var overlays: [String]?

    public init() {}
}
