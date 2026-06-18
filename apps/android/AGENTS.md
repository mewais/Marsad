# M02 — `apps/android` — working contract

Native Jetpack Compose companion (Tier UI). **SSH-only** — no local sessions. Binds the shared Rust
core via **UniFFI**-generated Kotlin bindings (`marsad-core`); native code is UI + thin platform glue
(FCM) only.

**Design source of truth:** `_extracted/design_handoff_marsad/Marsad Mobile.dc.html` (Android
variant: tab bar + FAB, Android radii). Build: bottom tabs **Sessions · Workspaces · Settings**;
session detail tabs; **gate = bottom sheet**; **diffs are unified (stacked)**; new-session bottom
sheet (SSH-only); terminal escape hatch with the **two-row accessory keypad** (sticky CTRL/ALT).
**Core API:** `docs/specs/core.md`. **Test plan:** `docs/specs/TESTPLAN.md`.

## Toolchain note
This block is **not built in the Linux scaffolding environment.** Its agents (test + code) run later
on an Android/Gradle toolchain. UniFFI Kotlin bindings are generated from `marsad-core` at assemble
time. (A `gradlew` wrapper is added on the toolchain machine.)

## What you may edit
- **M02 code-agent:** write `apps/android/app/src/main/**` (+ Gradle files).
- **M02 test-agent:** write `apps/android/app/src/test/**` and `app/src/androidTest/**`
  (JUnit/Robolectric + Compose UI tests), before the UI exists.
Do not edit other blocks.

## Definition of done (on the native toolchain)
- `./gradlew test connectedCheck` green; the interaction matrix in `docs/specs/TESTPLAN.md` is
  covered. App boots over the UniFFI core and drives a real remote session over SSH. UI fidelity to
  the mobile design is owner-verified.

## Rules
- Never parse terminal output. All data via the core's view-models/events.
- Keep logic in the Rust core; Kotlin is presentation + platform glue.
