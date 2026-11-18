import Foundation
import XCTest
@testable import NBEClient

/// Operator failure design, build step 2: the NO CONTACT rule. The shell
/// shows a value as live only while the newest frame was received less than
/// 2 s ago, measured on this machine's clock from the RECEIPT, never from the
/// tick's own stamp.
final class ContactAgeTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    func testNothingTrackedIsNotStale() {
        let c = ContactAge()
        XCTAssertNil(c.age(now: t0))
        XCTAssertFalse(c.isStale(now: t0), "no session (or the fixture): no curtain")
    }

    func testFreshUntilTwoSecondsThenStaleWithItsCount() {
        var c = ContactAge()
        c.received(tickTs: t0.timeIntervalSince1970 * 1000, at: t0)
        XCTAssertFalse(c.isStale(now: t0.addingTimeInterval(1.9)))
        XCTAssertFalse(c.isStale(now: t0.addingTimeInterval(2.0)), "2 s is the limit itself")
        XCTAssertTrue(c.isStale(now: t0.addingTimeInterval(3.0)), "3 s with nothing received")
        XCTAssertEqual(c.age(now: t0.addingTimeInterval(3.0)) ?? -1, 3.0, accuracy: 1e-9)
    }

    func testANewFrameClearsIt() {
        var c = ContactAge()
        c.received(at: t0)
        XCTAssertTrue(c.isStale(now: t0.addingTimeInterval(5)))
        c.received(at: t0.addingTimeInterval(5))
        XCTAssertFalse(c.isStale(now: t0.addingTimeInterval(5.1)))
    }

    /// The control plane's clock an hour ahead of this machine's (a shell on
    /// another Mac): age from the tick's own stamp would read negative and
    /// never go stale. Age from the receipt does.
    func testASenderClockAheadDoesNotKeepADeadLinkFresh() {
        var c = ContactAge()
        let aheadMs = (t0.timeIntervalSince1970 + 3600) * 1000
        c.received(tickTs: aheadMs, at: t0)
        XCTAssertTrue(c.isStale(now: t0.addingTimeInterval(3)), "3 s after the receipt is stale whatever the stamp says")
        XCTAssertEqual(c.age(now: t0.addingTimeInterval(3)) ?? -1, 3.0, accuracy: 1e-9)
    }

    func testResetStopsTracking() {
        var c = ContactAge()
        c.received(at: t0)
        c.reset()
        XCTAssertFalse(c.isStale(now: t0.addingTimeInterval(60)))
    }
}
