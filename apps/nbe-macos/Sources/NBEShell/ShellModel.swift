import Foundation
import NBEClient
import NBEProtocol
import SwiftUI

/// The shell's whole view of the show: the status snapshot and the latest
/// telemetry tick, as the control plane sent them. It decides nothing; a
/// button calls `run`, and what changes arrives as a `stateChange`.
@MainActor
public final class ShellModel: ObservableObject {
    public enum Connection: Equatable {
        case disconnected
        case connecting
        case connected(role: Role)
        /// The session ended under a connected shell (the control plane died
        /// or the socket dropped): the values shown are from before `since`,
        /// and no command can be sent.
        case lost(since: Date)
        case failed(String)
        case fixture
    }

    @Published public var connection: Connection = .disconnected
    @Published public var status = StatusSnapshot()
    @Published public var telemetry = Telemetry()
    /// From `show.getState` (finding 1's proposal); nil until it answers.
    @Published public var show: ShowStateView?
    @Published public var showNote: String?
    /// The last refusal or failure, as the server or the catalog worded it.
    @Published public var lastProblem: String?
    /// When the newest frame from the control plane was received (build
    /// step 2): past `ContactAge.limit` the NO CONTACT curtain is up, values
    /// are greyed, and no command can be sent.
    @Published public var contact = ContactAge()
    /// The shell's own clock, advanced four times a second so the age counts.
    @Published public var now = Date()
    /// The next reconnect attempt while lost (build step 3), for the curtain.
    @Published public var retryAttempt: Int?
    @Published public var log: [String] = []
    /// Commands sent and not yet answered, by name: each is pending, and is
    /// not sent again until its answer comes (the operator's double press).
    @Published public private(set) var inFlight: Set<String> = []

    public let catalog: CommandCatalog?
    private var session: ReconnectingSession?
    public private(set) var role: Role = .operator

    public init() {
        catalog = try? CommandCatalog.bundled()
        Task { [weak self] in
            while let self {
                self.now = Date()
                try? await Task.sleep(for: .milliseconds(250))
            }
        }
    }

    /// The NO CONTACT rule (build step 2): a session that has received
    /// nothing for `ContactAge.limit`.
    public var noContact: Bool { contact.isStale(now: now) }

    /// Seconds since the last frame, for the curtain's count.
    public var contactAgeSeconds: Int { Int((contact.age(now: now) ?? 0).rounded(.down)) }

    /// Whether this session's role may issue `command` (the §16.0 matrix, as
    /// exported). A button the role may not press is shown disabled, with why.
    public func may(_ command: ShellCommand) -> Bool {
        if inFlight.contains(command.name) { return false }
        if case .fixture = connection { return !noContact }
        guard case .connected = connection, !noContact else { return false }
        return catalog?[command.name]?.allows(role) ?? false
    }

    public func why(_ command: ShellCommand) -> String {
        if inFlight.contains(command.name) { return "\(command.name): pending (sent, no response yet)" }
        guard let desc = catalog?[command.name] else { return "\(command.name): not in the catalog" }
        return "\(command.name) — \(desc.roles.joined(separator: ", ")) (this session: \(role.rawValue))"
    }

    /// Connect, and keep reconnecting (build step 3): 0.25 s doubling to 2 s
    /// between attempts; on each return `system.status`, the telemetry
    /// subscription again (the session does both), then `show.getState`.
    public func connect(url: URL, token: String, role: Role) {
        self.role = role
        connection = .connecting
        if (try? ControlPlaneClient.checkTransport(url)) == nil {
            connection = .failed("a token over ws:// to a remote host is refused (SPEC §5.3: use wss://)")
            return
        }
        let session = ReconnectingSession(config: .init(url: url, token: token, role: role), catalog: catalog)
        self.session = session
        Task {
            await session.start()
            for await event in session.events {
                switch event {
                case .connected(let hello):
                    contact.received(at: Date())
                    if let data = hello.data, let snap = try? data.decode(StatusSnapshot.self) { status = snap }
                    connection = .connected(role: role)
                    retryAttempt = nil
                    note("connected")
                    await refreshShow()
                case .frame(let frame):
                    fold(frame)
                case .lost(let at):
                    if case .lost = connection {} else {
                        connection = .lost(since: at)
                        note("lost contact")
                    }
                case .retrying(let attempt, _):
                    retryAttempt = attempt
                }
            }
        }
    }

    /// A costly command waits here for the operator's confirmation.
    @Published public var awaitingConfirmation: ShellCommand?

    /// The question a costly command asks before it is sent; nil sends at
    /// once. Only stops that cannot be undone on air ask: TAKE, CUT and the
    /// rest stay one press, since speed matters there and the next press
    /// undoes them.
    public func confirmation(for command: ShellCommand) -> String? {
        switch command {
        case .showStop(let force):
            force ? "Force-stop the show? Outputs are cut without waiting."
                : "Stop the show? The clock stops and the View goes to its stop state."
        case .recordStop: "Stop the recording? The take ends here."
        case .streamStop: "Stop the stream? Viewers lose the programme."
        default: nil
        }
    }

    /// A press: a costly command waits for its confirmation; any other is sent.
    public func press(_ command: ShellCommand) {
        if confirmation(for: command) != nil {
            awaitingConfirmation = command
        } else {
            run(command)
        }
    }

    /// The `R` key: try again now instead of waiting out the backoff.
    public func retryNow() {
        guard let session else { return }
        Task { await session.retryNow() }
    }

    /// The fixture as last heard `staleSeconds` ago (the snapshot's
    /// `--stale N`, a look at the NO CONTACT curtain); nil leaves the fixture
    /// untracked, so an open `--fixture` window never goes stale.
    public func loadFixture(staleSeconds: Double? = nil, alerts: Bool = false) {
        if let staleSeconds { contact.received(at: Date().addingTimeInterval(-staleSeconds)) }
        connection = .fixture
        status = Fixture.status
        telemetry = alerts ? Fixture.telemetryWithAlerts : Fixture.telemetry
        show = Fixture.show
    }

    /// One of the fixture's states, for review (`--snapshot-all`, `--state`).
    public func loadFixture(state: FixtureState) {
        switch state {
        case .running: loadFixture()
        case .alerts: loadFixture(alerts: true)
        case .noContact: loadFixture(staleSeconds: 5)
        case .fallback:
            loadFixture()
            telemetry.fallbackActive = true
            status.fallbackActive = true
        case .lost:
            loadFixture()
            connection = .lost(since: Date().addingTimeInterval(-3))
        case .connecting:
            loadFixture()
            connection = .connecting
        case .failed:
            loadFixture()
            connection = .failed("sample failure: a token over ws:// to a remote host is refused")
        }
    }

    public func run(_ command: ShellCommand) {
        note("→ \(command.name) \(command.payload.isEmpty ? "" : String(describing: command.payload))")
        if case .fixture = connection { return }
        guard let session else { return }
        guard !inFlight.contains(command.name) else {
            note("\(command.name) already pending: not sent again")
            return
        }
        inFlight.insert(command.name)
        Task {
            defer { inFlight.remove(command.name) }
            do {
                let r = try await session.send(command)
                if let e = r.error { lastProblem = "\(command.name): \(e.code) \(e.message)" }
                note("← \(command.name) \(r.status) sv \(r.stateVersion)")
                // What the panels draw from show.getState (the rundown's
                // states, the graphics on air) follows every accepted command.
                if r.ok, command != .getState { await refreshShow() }
            } catch {
                lastProblem = "\(command.name): \(error)"
            }
        }
    }

    /// Ask `show.getState` again (after connect, and when a stateChange names
    /// the items or the package). A control plane without it says so once.
    public func refreshShow() async {
        guard let session else { return }
        guard let r = try? await session.send(.getState) else { return }
        if r.ok, let data = r.data, let view = try? data.decode(ShowStateView.self) {
            show = view
            showNote = nil
        } else {
            showNote = "show.getState: \(r.error?.code ?? "no data") — the rundown needs it (finding 1)"
        }
    }

    private func fold(_ frame: InboundFrame) {
        switch frame {
        case .stateChange(let change):
            contact.received(at: Date())
            status.apply(change)
            if change.changed?.contains(where: { ["itemStates", "package", "showState"].contains($0) }) ?? true {
                Task { await refreshShow() }
            }
        case .telemetry(let t):
            contact.received(tickTs: t.data.ts, at: Date())
            telemetry = t.data
        case .response, .unknown: break
        }
    }

    private func note(_ line: String) {
        log.insert(line, at: 0)
        if log.count > 200 { log.removeLast() }
    }
}

/// The states the fixture can show, one picture each (`--snapshot-all`).
public enum FixtureState: String, CaseIterable, Sendable {
    case running, alerts, noContact, fallback, lost, connecting, failed
}

/// What the shell draws with no control plane (`--fixture`): the spec's own
/// §10.1 example and a running show's snapshot.
public enum Fixture {
    public static let telemetry: Telemetry = telemetry(alerts: "[]")

    /// Three sample alerts, to see the banner (`--alerts`): a critical one, a
    /// warning, and an id this shell does not know.
    public static let alerts = """
    [{"id":"item.failed","severity":"critical","reason":"held: E_CLIP_DECODE: clip A1 stopped decoding at source frame 42","since":1768000000000,"auditEvent":"item.failed"},
     {"id":"audio.underrun","severity":"warning","reason":"3 underruns in the last 10 s","since":1768000000000,"auditEvent":"audio.underrun"},
     {"id":"sample.unknown","severity":"warning","reason":"a sample id the shell does not know","since":1768000000000,"auditEvent":null}]
    """

    public static let telemetryWithAlerts: Telemetry = telemetry(alerts: alerts)

    static func telemetry(alerts: String) -> Telemetry {
        let json = """
        {"ts":1768000000000,"showState":"RUNNING","masterClockFrame":54000,"viewItem":"B2","previewItem":"B3",
         "droppedFramesTotal":0,"renderGpuTimeMs":4.7,"decodeSessions":4,"streamState":"live","streamBufferMs":210,
         "streamTransportState":"live","recordState":"recording","recordSpaceMib":512000,"masterClockDriftMs":0.2,
         "fallbackActive":false,"qualityProfile":"consumer","degradationRung":0,"watchdogTripsTotal":0,
         "watchdogClearsTotal":0,"readAheadMissesTotal":0,"automationHold":false,"audioUnderrunsTotal":0,
         "audioDriftMs":0.4,"busPeakDbfs":{"master":-12.3,"mic":-18.0},
         "audioDevice":{"state":"open","name":"MacBook Pro Speakers","rateHz":48000,"reason":null},
         "recordTapPath":"zeroCopy","alerts":\(alerts)}
        """
        return (try? JSONDecoder().decode(Telemetry.self, from: Data(json.utf8))) ?? Telemetry()
    }

    public static let show: ShowStateView = {
        let json = """
        {"stateVersion":413,"showState":"RUNNING","rundown":{"id":"rundown","items":[
          {"id":"A1","kind":"clip","durationFrames":150,"autoFollow":true,"state":"DONE"},
          {"id":"B2","kind":"scene","durationFrames":null,"autoFollow":false,"state":"LIVE"},
          {"id":"B3","kind":"scene","durationFrames":null,"autoFollow":false,"state":"ARMED"},
          {"id":"C1","kind":"clip","durationFrames":300,"autoFollow":false,"state":"READY"},
          {"id":"C2","kind":"clip","durationFrames":90,"autoFollow":false,"state":"MISSING"}]},
         "elements":[{"id":"lt1","kind":"lowerThird"},{"id":"bug","kind":"image"}],"overlays":["lt1","bug"]}
        """
        return (try? JSONDecoder().decode(ShowStateView.self, from: Data(json.utf8))) ?? ShowStateView()
    }()

    public static let status: StatusSnapshot = {
        let json = """
        {"showState":"RUNNING","package":{"path":"/shows/dress"},"preflightPassed":true,"masterClockState":"RUNNING",
         "stateVersion":413,"viewItem":"B2","previewItem":"B3","visibleOverlays":["lt1"],"fallbackActive":false,
         "recordState":"recording","streamState":"live","automationHold":false,"qualityProfile":"consumer",
         "lastError":null,"alerts":[]}
        """
        return (try? JSONDecoder().decode(StatusSnapshot.self, from: Data(json.utf8))) ?? StatusSnapshot()
    }()
}
