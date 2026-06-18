# M01 — `apps/ios` — working contract

Native SwiftUI companion (Tier UI). **SSH-only** — no local sessions. Binds the shared Rust core via
**UniFFI**-generated Swift bindings (`marsad-core`); native code is UI + thin platform glue (APNs)
only.

**Design source of truth:** `_extracted/design_handoff_marsad/Marsad Mobile.dc.html` — phone-
appropriate language; device frame with iOS/Android specifics. Build: bottom tabs **Sessions ·
Workspaces · Settings**; session detail tabs (Activity/Changes/Config/Usage/Terminal); **gate =
bottom sheet** (permission/plan/edit, approve / reject-with-reason); **diffs are unified (stacked)**;
new-session = bottom sheet (SSH-only); terminal escape hatch with the **two-row accessory keypad**
(ESC / | - Home ↑ End PgUp · TAB CTRL ALT ← ↓ → PgDn ⌨) where CTRL/ALT are **sticky modifiers**.
**Core API:** `docs/specs/core.md`. **Test plan:** `docs/specs/TESTPLAN.md`.

## Toolchain note
This block is **not built in the Linux scaffolding environment.** Its agents (test + code) run later
on a macOS/Xcode toolchain. `xcodegen generate` produces the `.xcodeproj` from `project.yml`. The
UniFFI Swift bindings are generated from `marsad-core` as part of the build.

## What you may edit
- **M01 code-agent:** write `apps/ios/Marsad/**` (+ `project.yml`).
- **M01 test-agent:** write `apps/ios/MarsadTests/**` (XCTest + ViewInspector), before the UI exists.
Do not edit other blocks.

## Definition of done (on the native toolchain)
- `xcodegen generate && xcodebuild test` green; the interaction matrix in `docs/specs/TESTPLAN.md` is
  covered. App boots over the UniFFI core and drives a real remote session over SSH. UI fidelity to
  the mobile design is owner-verified.

## Rules
- Never parse terminal output. All data via the core's view-models/events.
- Keep logic in the Rust core; Swift is presentation + platform glue.
