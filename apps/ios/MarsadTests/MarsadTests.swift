import XCTest
@testable import Marsad

// M01 test suite (run on a Mac toolchain). Mirrors the UI interaction matrix in
// docs/specs/TESTPLAN.md: gate sheet approve/reject routes to one session, terminal
// keypad sticky CTRL/ALT modifiers, push-tap routing, unified diff render, filters.
//
// TODO(M01 test-agent): author with XCTest + ViewInspector before the UI exists.
final class MarsadTests: XCTestCase {
    func testPlaceholder() throws {
        // Intentionally failing/absent until the UI is implemented on the native toolchain.
        throw XCTSkip("M01 UI not yet implemented — author real tests on the Mac toolchain.")
    }
}
