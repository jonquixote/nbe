import Foundation
import NBEProtocol
import XCTest

@testable import NBEShell

/// The operator shell's model, now importable (the NBEShell library target).
@MainActor
final class ShellModelTests: XCTestCase {
    /// #122's surviving mutant MC (`may()` ignoring the stale state), killed:
    /// a session last heard 5 s ago is past the 2 s contact limit, so no
    /// command may be sent; heard just now, the same commands may.
    func testNoCommandMayBeSentWhileContactIsStale() {
        let model = ShellModel()
        model.loadFixture(staleSeconds: 5)
        model.now = Date()
        XCTAssertTrue(model.noContact, "5 s without a frame is NO CONTACT")
        for command in ShellCommand.samples {
            XCTAssertFalse(model.may(command), "\(command.name) must be refused while there is no contact")
        }
        model.loadFixture(staleSeconds: 0)
        model.now = Date()
        XCTAssertFalse(model.noContact)
        XCTAssertTrue(
            ShellCommand.samples.allSatisfy { model.may($0) },
            "with contact, the fixture session may send every sample command"
        )
    }
}
