import Foundation
import XCTest
@testable import NBEProtocol

/// Operator failure design, build step 4 (row A3): the banner shows the
/// tick's alerts. An alert raised between commands arrives on the next tick,
/// with no `stateChange`; the snapshot's list waited for the next accepted
/// command.
final class AlertFeedTests: XCTestCase {
    private func decode<T: Decodable>(_ json: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }

    private let critical = #"{"id":"engine.notReporting","severity":"critical","reason":"no engine report for 2 s","since":1800000000000,"auditEvent":null}"#

    func testATickRaisesAnAlertWithNoStateChange() throws {
        let status: StatusSnapshot = try decode(#"{"showState":"RUNNING","alerts":[]}"#)
        let tick: Telemetry = try decode(#"{"ts":1800000000000,"alerts":[\#(critical)]}"#)
        let shown = AlertFeed.current(status: status, telemetry: tick)
        XCTAssertEqual(shown.map(\.id), ["engine.notReporting"], "the tick's alert is shown although the snapshot says none")
        XCTAssertEqual(shown.first?.severity, "critical")
    }

    func testATickClearsAnAlertTheSnapshotStillHolds() throws {
        let status: StatusSnapshot = try decode(#"{"showState":"RUNNING","alerts":[\#(critical)]}"#)
        let tick: Telemetry = try decode(#"{"ts":1800000000000,"alerts":[]}"#)
        XCTAssertEqual(AlertFeed.current(status: status, telemetry: tick), [], "cleared when the tick's list no longer holds it")
    }

    func testBeforeTheFirstTickTheSnapshotIsUsed() throws {
        let status: StatusSnapshot = try decode(#"{"showState":"RUNNING","alerts":[\#(critical)]}"#)
        XCTAssertEqual(AlertFeed.current(status: status, telemetry: Telemetry()).map(\.id), ["engine.notReporting"])
        XCTAssertEqual(AlertFeed.current(status: StatusSnapshot(), telemetry: Telemetry()), [])
    }
}
