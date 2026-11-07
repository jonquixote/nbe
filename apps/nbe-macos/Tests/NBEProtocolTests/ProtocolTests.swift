import Foundation
import XCTest
@testable import NBEClient
@testable import NBEProtocol

private func fixture(_ name: String) throws -> Data {
    let url = try XCTUnwrap(Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Fixtures"))
    return try Data(contentsOf: url)
}

/// The frames the shell reads, decoded from the spec's own examples (Prompt
/// 13 step 4.1: fixture-driven).
final class FrameTests: XCTestCase {
    func testTheSpecsTelemetryExampleDecodes() throws {
        guard case .telemetry(let t) = try InboundFrame.decode(fixture("telemetry-spec-10.1")) else {
            return XCTFail("a telemetry frame")
        }
        XCTAssertEqual(t.data.showState, "RUNNING")
        XCTAssertEqual(t.data.masterClockFrame, 54000)
        XCTAssertEqual(t.data.droppedFramesTotal, 0)
        XCTAssertEqual(t.data.busPeakDbfs?["master"] ?? nil, -12.3)
        XCTAssertEqual(t.data.audioDevice?.state, "open")
        XCTAssertEqual(t.data.alerts, [])
    }

    func testAStateChangeFoldsIntoTheSnapshot() throws {
        guard case .stateChange(let change) = try InboundFrame.decode(fixture("state-change")) else {
            return XCTFail("a stateChange frame")
        }
        var snap = StatusSnapshot()
        snap.showState = "RUNNING"
        snap.viewItem = "B2"
        snap.previewItem = "B3"
        snap.apply(change)
        XCTAssertEqual(snap.viewItem, "B3")
        XCTAssertNil(snap.previewItem, "a key set to null is cleared, not kept")
        XCTAssertEqual(snap.showState, "RUNNING", "a key the change does not name is kept")
        XCTAssertEqual(snap.stateVersion, 414)
        XCTAssertEqual(snap.alerts?.first?.id, "outputs.held")
    }

    func testResponsesAreToldFromPushesByRequestId() throws {
        guard case .response(let ok) = try InboundFrame.decode(fixture("response-ok")) else {
            return XCTFail("a response")
        }
        XCTAssertTrue(ok.ok)
        guard case .response(let err) = try InboundFrame.decode(fixture("response-error")) else {
            return XCTFail("a response")
        }
        XCTAssertFalse(err.ok)
        XCTAssertEqual(err.error?.code, "E_FORBIDDEN_STATE")
    }

    func testAnUnknownPushKindIsKeptNotFatal() throws {
        let frame = try InboundFrame.decode(Data(#"{"v":"0.3","kind":"somethingNew","x":1}"#.utf8))
        XCTAssertEqual(frame, .unknown(kind: "somethingNew"))
    }

    func testTheEnvelopeCarriesNoKindAndOmitsAnAbsentBaseVersion() throws {
        let env = CommandEnvelope(id: "id-1", command: "view.take", payload: ["transition": "cut"])
        let obj = try JSONDecoder().decode([String: JSONValue].self, from: JSONEncoder().encode(env))
        XCTAssertEqual(obj["v"], "0.3")
        XCTAssertNil(obj["kind"], "§5.4.1 rule 4: a client envelope never carries kind")
        XCTAssertNil(obj["baseStateVersion"])
    }

    func testAnIntegralNumberGoesOutAsAnInteger() throws {
        let data = try JSONEncoder().encode(JSONValue.object(["durationFrames": 12]))
        XCTAssertEqual(String(decoding: data, as: UTF8.self), #"{"durationFrames":12}"#)
    }
}

/// Prompt 13 step 3 and step 4.2: every binding against the §16 catalog the
/// control plane exports, so a changed command fails here, not at showtime.
final class ParityTests: XCTestCase {
    var catalog: CommandCatalog!

    override func setUpWithError() throws {
        catalog = try CommandCatalog.bundled()
    }

    func testEveryBindingIsACommandTheCatalogAccepts() {
        for command in ShellCommand.samples {
            XCTAssertEqual(catalog.problems(command: command.name, payload: command.payload), [],
                           "\(command.name) \(command.payload)")
        }
    }

    func testTheCheckRefusesWhatTheServerWould() {
        XCTAssertFalse(catalog.problems(command: "view.take", payload: ["transition": "fade"]).isEmpty,
                       "an enum value the schema does not list")
        XCTAssertFalse(catalog.problems(command: "view.take", payload: ["bogus": true]).isEmpty,
                       "a key a strict payload does not take")
        XCTAssertFalse(catalog.problems(command: "preview.set", payload: [:]).isEmpty, "a required key missing")
        XCTAssertFalse(catalog.problems(command: "view.take", payload: ["durationFrames": 1.5]).isEmpty,
                       "a fraction where an integer is required")
        XCTAssertFalse(catalog.problems(command: "view.take", payload: ["durationFrames": 601]).isEmpty,
                       "past the schema's max")
        XCTAssertFalse(catalog.problems(command: "show.getEverything", payload: [:]).isEmpty,
                       "a command §16 does not have")
        // Finding 1's proposal, on the draft branch's control plane: read-only, every role.
        XCTAssertEqual(catalog["show.getState"]?.roles, ["monitor", "operator", "producer", "admin"])
    }

    func testTheMatrixIsTheCatalogs() {
        XCTAssertEqual(catalog["show.start"]?.roles, ["admin"], "§16.0: show.start is admin-only")
        XCTAssertEqual(catalog["show.stop"]?.allows(.operator), true, "§16.0: the operator may stop")
        XCTAssertEqual(catalog["show.load"]?.allows(.operator), false, "§16.0: load is the producer's")
        XCTAssertEqual(catalog["system.status"]?.allows(.monitor), true)
        XCTAssertFalse(catalog.problems(command: "show.start", payload: [:], role: .operator).isEmpty)
    }
}

/// AC-16: each of the ten actions maps to commands the catalog has, and the
/// roles they need are named (the draft's finding 2: no one role short of
/// admin does all ten).
final class AC16Tests: XCTestCase {
    func testEachActionIsBoundAndWhoMayDoItIsKnown() throws {
        let catalog = try CommandCatalog.bundled()
        var needs: [String: Set<String>] = [:]
        for action in AC16Action.allCases {
            XCTAssertFalse(action.commands.isEmpty, action.rawValue)
            for c in action.commands {
                let desc = try XCTUnwrap(catalog[c.name], "\(action.rawValue): \(c.name)")
                XCTAssertEqual(catalog.problems(command: c.name, payload: c.payload), [], "\(action.rawValue)")
                needs[action.rawValue, default: []].formUnion(desc.roles)
            }
        }
        let allTen = needs.values.reduce(Set(Role.allCases.map(\.rawValue))) { $0.intersection($1) }
        XCTAssertEqual(allTen, ["admin"], "only admin may do all ten AC-16 actions under §16.0")
    }
}

final class HandshakeTests: XCTestCase {
    func testTheUpgradeCarriesSection53sHeaders() {
        let req = ControlPlaneClient.handshake(.local(token: "t0k", role: .operator))
        XCTAssertEqual(req.url?.absoluteString, "ws://127.0.0.1:8462/nbe/v0.3")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer t0k")
        XCTAssertEqual(req.value(forHTTPHeaderField: "X-NBE-Role"), "operator")
    }

    func testATokenNeverTravelsInCleartextToAnotherHost() async throws {
        XCTAssertNoThrow(try ControlPlaneClient.checkTransport(URL(string: "ws://127.0.0.1:8462/nbe/v0.3")!))
        XCTAssertNoThrow(try ControlPlaneClient.checkTransport(URL(string: "ws://localhost:8462/nbe/v0.3")!))
        XCTAssertNoThrow(try ControlPlaneClient.checkTransport(URL(string: "wss://render.local:8463/nbe/v0.3")!))
        XCTAssertThrowsError(try ControlPlaneClient.checkTransport(URL(string: "ws://render.local:8462/nbe/v0.3")!))
        let client = ControlPlaneClient(config: .init(url: URL(string: "ws://10.0.0.5:8462/nbe/v0.3")!, token: "t", role: .operator))
        do {
            _ = try await client.connect()
            XCTFail("a token must not leave over ws:// to another host")
        } catch let ControlPlaneClient.ClientError.cleartextToRemoteHost(host) {
            XCTAssertEqual(host, "10.0.0.5")
        }
    }

    func testTheClientRefusesABindingTheRoleMayNotSendBeforeTheWire() async {
        let client = ControlPlaneClient(config: .local(token: "t", role: .operator))
        do {
            _ = try await client.send(.showStart)
            XCTFail("show.start from an operator must not leave the client")
        } catch let ControlPlaneClient.ClientError.refusedByCatalog(problems) {
            XCTAssertTrue(problems.first?.contains("admin") == true, "\(problems)")
        } catch {
            XCTFail("\(error)")
        }
    }
}
