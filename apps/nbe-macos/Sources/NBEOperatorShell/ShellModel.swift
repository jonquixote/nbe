import Foundation
import NBEClient
import NBEProtocol
import SwiftUI

/// The shell's whole view of the show: the status snapshot and the latest
/// telemetry tick, as the control plane sent them. It decides nothing; a
/// button calls `run`, and what changes arrives as a `stateChange`.
@MainActor
final class ShellModel: ObservableObject {
    enum Connection: Equatable {
        case disconnected
        case connecting
        case connected(role: Role)
        case failed(String)
        case fixture
    }

    @Published var connection: Connection = .disconnected
    @Published var status = StatusSnapshot()
    @Published var telemetry = Telemetry()
    /// The last refusal or failure, as the server or the catalog worded it.
    @Published var lastProblem: String?
    @Published var log: [String] = []

    let catalog: CommandCatalog?
    private var client: ControlPlaneClient?
    private(set) var role: Role = .operator

    init() {
        catalog = try? CommandCatalog.bundled()
    }

    /// Whether this session's role may issue `command` (the §16.0 matrix, as
    /// exported). A button the role may not press is shown disabled, with why.
    func may(_ command: ShellCommand) -> Bool {
        if case .fixture = connection { return true }
        guard case .connected = connection else { return false }
        return catalog?[command.name]?.allows(role) ?? false
    }

    func why(_ command: ShellCommand) -> String {
        guard let desc = catalog?[command.name] else { return "\(command.name): not in the catalog" }
        return "\(command.name) — \(desc.roles.joined(separator: ", ")) (this session: \(role.rawValue))"
    }

    func connect(url: URL, token: String, role: Role) {
        self.role = role
        connection = .connecting
        let client = ControlPlaneClient(config: .init(url: url, token: token, role: role), catalog: catalog)
        self.client = client
        Task {
            do {
                let response = try await client.connect()
                if let data = response.data, let snap = try? data.decode(StatusSnapshot.self) { status = snap }
                connection = .connected(role: role)
                _ = try? await client.send(.telemetrySubscribe(intervalMs: 100))
                for await frame in client.pushes { fold(frame) }
                connection = .disconnected
            } catch {
                connection = .failed(String(describing: error))
            }
        }
    }

    func loadFixture() {
        connection = .fixture
        status = Fixture.status
        telemetry = Fixture.telemetry
    }

    func run(_ command: ShellCommand) {
        note("→ \(command.name) \(command.payload.isEmpty ? "" : String(describing: command.payload))")
        if case .fixture = connection { return }
        guard let client else { return }
        Task {
            do {
                let r = try await client.send(command, baseStateVersion: nil)
                if let e = r.error { lastProblem = "\(command.name): \(e.code) \(e.message)" }
                note("← \(command.name) \(r.status) sv \(r.stateVersion)")
            } catch {
                lastProblem = "\(command.name): \(error)"
            }
        }
    }

    private func fold(_ frame: InboundFrame) {
        switch frame {
        case .stateChange(let change): status.apply(change)
        case .telemetry(let t): telemetry = t.data
        case .response, .unknown: break
        }
    }

    private func note(_ line: String) {
        log.insert(line, at: 0)
        if log.count > 200 { log.removeLast() }
    }
}

/// What the shell draws with no control plane (`--fixture`): the spec's own
/// §10.1 example and a running show's snapshot.
enum Fixture {
    static let telemetry: Telemetry = {
        let json = """
        {"ts":1768000000000,"showState":"RUNNING","masterClockFrame":54000,"viewItem":"B2","previewItem":"B3",
         "droppedFramesTotal":0,"renderGpuTimeMs":4.7,"decodeSessions":4,"streamState":"live","streamBufferMs":210,
         "streamTransportState":"live","recordState":"recording","recordSpaceMib":512000,"masterClockDriftMs":0.2,
         "fallbackActive":false,"qualityProfile":"consumer","degradationRung":0,"watchdogTripsTotal":0,
         "watchdogClearsTotal":0,"readAheadMissesTotal":0,"automationHold":false,"audioUnderrunsTotal":0,
         "audioDriftMs":0.4,"busPeakDbfs":{"master":-12.3,"mic":-18.0},
         "audioDevice":{"state":"open","name":"MacBook Pro Speakers","rateHz":48000,"reason":null},
         "recordTapPath":"zeroCopy","alerts":[]}
        """
        return (try? JSONDecoder().decode(Telemetry.self, from: Data(json.utf8))) ?? Telemetry()
    }()

    static let status: StatusSnapshot = {
        let json = """
        {"showState":"RUNNING","package":{"path":"/shows/dress"},"preflightPassed":true,"masterClockState":"RUNNING",
         "stateVersion":413,"viewItem":"B2","previewItem":"B3","visibleOverlays":["lt1"],"fallbackActive":false,
         "recordState":"recording","streamState":"live","automationHold":false,"qualityProfile":"consumer",
         "lastError":null,"alerts":[]}
        """
        return (try? JSONDecoder().decode(StatusSnapshot.self, from: Data(json.utf8))) ?? StatusSnapshot()
    }()
}
