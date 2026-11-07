import Foundation
import XCTest
@testable import NBEClient
@testable import NBEProtocol

/// The client against a real control plane (opt-in: skipped unless
/// `NBE_SHELL_E2E_URL`, `NBE_SHELL_E2E_OPERATOR_TOKEN` and
/// `NBE_SHELL_E2E_ADMIN_TOKEN` are set; `scripts/shell-e2e.sh` starts one).
/// No engine is attached: the server answers from its own state.
final class LiveControlPlaneTests: XCTestCase {
    private var env: (url: URL, op: String, admin: String)!

    override func setUpWithError() throws {
        let e = ProcessInfo.processInfo.environment
        guard let u = e["NBE_SHELL_E2E_URL"], let url = URL(string: u),
              let op = e["NBE_SHELL_E2E_OPERATOR_TOKEN"], let admin = e["NBE_SHELL_E2E_ADMIN_TOKEN"]
        else { throw XCTSkip("no control plane: NBE_SHELL_E2E_URL and the two tokens are unset") }
        env = (url, op, admin)
    }

    private func client(_ token: String, _ role: Role) -> ControlPlaneClient {
        ControlPlaneClient(config: .init(url: env.url, token: token, role: role, responseGuard: .seconds(5)))
    }

    func testAnOperatorConnectsAndReadsTheStatusSnapshot() async throws {
        let c = client(env.op, .operator)
        let r = try await c.connect()
        XCTAssertTrue(r.ok, "system.status: \(r)")
        let snap = try XCTUnwrap(r.data).decode(StatusSnapshot.self)
        XCTAssertNotNil(snap.showState, "the snapshot names the show's state")
        XCTAssertNotNil(snap.stateVersion)
        await c.disconnect()
    }

    func testTheServerRefusesWhatTheStateForbidsAndSaysWhy() async throws {
        let c = client(env.admin, .admin)
        _ = try await c.connect()
        // No package is loaded: a start is refused by the server, not the client.
        let r = try await c.send(.showStart)
        XCTAssertFalse(r.ok)
        XCTAssertNotNil(r.error?.code, "\(r)")
        await c.disconnect()
    }

    func testTelemetryArrivesOnTheSubscribedInterval() async throws {
        let c = client(env.op, .operator)
        _ = try await c.connect()
        let r = try await c.send(.telemetrySubscribe(intervalMs: 100))
        XCTAssertTrue(r.ok, "\(r)")
        // A hang guard, not a measurement: the first tick, or the test fails.
        let got = await withTaskGroup(of: Telemetry?.self) { group -> Telemetry? in
            group.addTask {
                for await frame in c.pushes { if case .telemetry(let t) = frame { return t.data } }
                return nil
            }
            group.addTask { try? await Task.sleep(for: .seconds(10)); return nil }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first
        }
        let t = try XCTUnwrap(got, "no telemetry frame within the 10 s guard")
        XCTAssertNotNil(t.showState)
        await c.disconnect()
    }

    func testABadTokenIsRefusedAtTheHandshake() async throws {
        let c = client("not-a-token", .operator)
        do {
            _ = try await c.connect()
            XCTFail("a bad token must not connect")
        } catch {
            // §5.3: refused before the upgrade (401); the client sees the socket fail.
        }
        await c.disconnect()
    }
}
