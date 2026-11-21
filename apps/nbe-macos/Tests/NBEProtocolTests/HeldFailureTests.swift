import Foundation
import NBEProtocol
import XCTest

/// show.getState's held failure per item (the user's ruling of 2026-11-18 07:28).
final class HeldFailureTests: XCTestCase {
    private func item(_ json: String) throws -> ShowStateView.Item {
        try JSONDecoder().decode(ShowStateView.Item.self, from: Data(json.utf8))
    }

    /// S1: the field decodes, and its absence is nil.
    func testTheHeldFailureDecodes() throws {
        let held = try item(#"{"id":"A1","kind":"clip","durationFrames":150,"autoFollow":false,"state":"ERROR","heldFailure":"held: E_CLIP_DECODE: clip A1 stopped at 42"}"#)
        XCTAssertEqual(held.heldFailure, "held: E_CLIP_DECODE: clip A1 stopped at 42")
        let plain = try item(#"{"id":"A1","kind":"clip","durationFrames":150,"autoFollow":false,"state":"READY"}"#)
        XCTAssertNil(plain.heldFailure)
    }

    /// S2: the rundown's label carries the held failure's words.
    func testTheRundownLabelCarriesTheHeldWords() throws {
        let held = try item(#"{"id":"A1","kind":"clip","durationFrames":150,"autoFollow":false,"state":"ERROR","heldFailure":"held: E_CLIP_DECODE"}"#)
        XCTAssertEqual(held.stateLabel, "ERROR, held: E_CLIP_DECODE")
        let plain = try item(#"{"id":"A1","kind":"clip","durationFrames":150,"autoFollow":false,"state":"ERROR","heldFailure":null}"#)
        XCTAssertEqual(plain.stateLabel, "ERROR")
    }
}
