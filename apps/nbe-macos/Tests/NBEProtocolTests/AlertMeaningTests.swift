import Foundation
import XCTest

@testable import NBEShell

final class AlertMeaningTests: XCTestCase {
    /// A1, the drift check: every alert id the control plane raises has a
    /// meaning in the shell. Reads alerts.ts from this repository.
    func testEveryAlertTheControlPlaneRaisesHasAMeaning() throws {
        let repo = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // NBEProtocolTests
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // nbe-macos
            .deletingLastPathComponent() // apps
            .deletingLastPathComponent() // the repository
        let source = try String(
            contentsOf: repo.appendingPathComponent("packages/control-plane/src/alerts.ts"), encoding: .utf8)
        let regex = try NSRegularExpression(pattern: #"\bid: "([a-zA-Z.]+)""#)
        let ids = Set(regex.matches(in: source, range: NSRange(source.startIndex..., in: source)).compactMap {
            Range($0.range(at: 1), in: source).map { String(source[$0]) }
        })
        XCTAssertFalse(ids.isEmpty, "alerts.ts names its alerts as id: \"...\"")
        let missing = ids.subtracting(AlertMeaning.table.keys).sorted()
        XCTAssertEqual(missing, [], "alert ids with no meaning in the shell: \(missing)")
    }

    /// A2: an id the shell does not know is shown as it came.
    func testAnUnknownAlertIsShownByItsId() {
        let e = AlertMeaning.of("something.new")
        XCTAssertEqual(e.title, "something.new")
        XCTAssertTrue(e.meaning.contains("does not know"))
    }

    /// #133: ticker.notOnAir resolves to its own entry, not to the unknown-id
    /// fallback A2 covers.
    func testTheTickerItemCountMismatchHasItsOwnEntry() {
        let e = AlertMeaning.of("ticker.notOnAir")
        XCTAssertEqual(
            e,
            AlertMeaning.Entry(
                title: "TICKER ITEM COUNT MISMATCH",
                meaning: "After the settling period, the engine reports a different ticker override item count from the control plane.",
                action: "Check the programme output and engine log before changing or clearing the override."))
        XCTAssertFalse(e.meaning.contains("does not know"), "the known id must not fall back to the unknown entry")
    }
}
