import SwiftUI

// M01 — iOS app entry point.
//
// TODO(M01 code-agent): build the SSH-only mobile UI over the UniFFI-generated
// `marsad-core` bindings, faithful to _extracted/design_handoff_marsad/Marsad Mobile.dc.html:
// bottom tabs (Sessions/Workspaces/Settings), session detail tabs, gate BOTTOM SHEET,
// unified (stacked) diffs, terminal accessory keypad with sticky CTRL/ALT, APNs glue.
// See apps/ios/AGENTS.md and docs/specs/core.md.

@main
struct MarsadApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

struct ContentView: View {
    var body: some View {
        VStack(spacing: 8) {
            Text("Marsad — مرصد").font(.title.bold())
            Text("Scaffolding. UI not yet implemented — see apps/ios/AGENTS.md.")
                .foregroundStyle(.secondary)
        }
        .padding()
    }
}
