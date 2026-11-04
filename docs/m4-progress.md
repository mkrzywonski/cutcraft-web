# Independent M4 implementation

This branch is **in progress**, not an M4 completion or merge approval. It starts at M3
`b9ea099` and carries the revised design/parity documents from `ffc8821`.
The reference remains CutCraft 1.0.5 (`39654a9465ecdef7478703ee586276304dbc29c6`).

## Working foundation

The server bundles an egui/WebAssembly desktop at `/desktop`, with the reference vector
icons and panel headers. Both desktop and existing station pages use the service's shared
project and cutter state. Desktop edits carry the session incarnation and revision;
Stop bypasses the browser mutation queue.

The workspace includes the menu/toolbar, three views and ten panel groups, readable
preparation choices, material/mat libraries and editors, source rendering, local vector
drawing, media/design/test positioning, media/design resizing, material adjustment,
Save/Save As, download/export, library folders, and separate cut/test screens.

Saving no longer depends on cut readiness. Embedded project sources are rebound into the
owner's private area without rewriting uploaded bytes. Undo/Redo survive worker recovery.
A test-only frozen copy can be prepared when the artwork has unresolved questions; it
cannot be used for a main cut. Preview and execution share test-placement planning.

## Repeatable checks

From the pinned `nix-shell`:

```sh
scripts/build-desktop.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --locked -p cutcraft-ui --target wasm32-unknown-unknown -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
scripts/desktop-browser-check.sh
```

The browser check uses authored artwork and a fresh practice-only service. It exercises
typed dimensions, material force, first Save opening Save As, clean view navigation, drag
resizing/Undo, explicit test preparation, two test cuts followed by a design cut on the
same loaded mat, and reload without restarting motion. It captures 1280×800 and
1920×1080 views in its scratch directory. This is interaction evidence, **not** the full
paired reference screenshot review.

## Remaining completion gates

These are required work, not exclusions or waived acceptance:

- Finish advanced preparation controls and isolated font management; exercise all authored
  regression fixtures and Show me cases.
- Complete region-at-zoom artwork rendering, rulers, gesture/settling parity, occupied test
  overlays, and cut-order playback fidelity.
- Finish cutter recovery/finished states and preparation replacement after settings edits;
  verify actor changes, unknown Start outcomes, reload during cuts, and immediate Stop.
- Complete picker/save conflicts/default editing, account integration, error presentation,
  keyboard accessibility and large-font/narrow-desktop layouts.
- Verify settings scope, new-design defaults, preset adoption, two-device updates and
  two-person isolation through browser interactions.
- Finish asset versioning/compression, command parity inventory, dependency policy,
  compact-layout regression checks and clean-checkout packaging.
- Paired reference screenshots and the complete acceptance catalogue; independent review
  of exact commits. Pi performance, Safari, hardware and release gates remain unverified.

Do not start M5 or merge this branch as a completed M4 while these requirements remain.
No real cutter has been moved for this implementation.

## Foundation checkpoint evidence (2026-10-29)

On the development workstation, version 0.14.0 passed formatting, native workspace
Clippy, browser-target Clippy for UI/protocol, all **449 Rust tests**, the workspace build,
`cargo deny check`, **40 existing compact-browser assertions**, and the desktop workflow
script above. The two history/upload-preservation regressions pass as part of that suite.
Checks used only authored fixtures and the simulator. The remaining gates above are still open.

## Preview ownership follow-up

Private cut-copy previews now require their owner's authenticated stable principal. An HTTP
regression queues an authored design as one account and checks that a second account receives
404 with no geometry. Browser transport regressions cover waiting behind someone else's turn,
a refused preview without losing the workspace connection, discarding a previous session's
preview, and avoiding replay of an old Save result into a later dialog.

Version 0.14.1 passed formatting, native and wasm Clippy, all 450 Rust tests, workspace
build, dependency policy, the desktop simulated workflow and 40 compact-browser assertions.
The transport regression script also passed after adding the session-change case. These checks
ran locally against authored fixtures; they do not close the remaining M4 gates above.

## Prepared cut recovery

The desktop can recover the current owned cut copy through the read-only preview response.
Reloading before loading or between successful tests does not enqueue another turn or start
motion. Cutter selection is a local display preference; Start still uses the current server
boot, turn, revision, mat generation and action epoch. Waiting copies show their queue state.
The desktop browser workflow now reloads both before loading and after the first test and
checks that the turn and occupied test area survive.

This does not yet close recovery acceptance: active-motion reload, unknown Start outcomes,
Stop/account changes, and replacing a prepared copy after edits still need their complete
implementation and failure tests. Replacement must compare source/session identity as well as
project revision; a revision number alone is not an identity for a different opened design.

Version 0.14.2 also passed formatting, native/wasm Clippy, all 450 Rust tests, workspace
build and dependency policy. The expanded desktop workflow passed both reload points and
continued through two tests and a design cut on the same simulated mat. Transport tests cover
read-only recovery and dropping an in-flight preview after switching cutters. Compact-browser
checks last passed at 0.14.1; the recovery change does not modify the compact client.

## Comparison canvas follow-up (2026-11-04)

Addressed Zulip #126: the checkerboard now uses its pane's paint clip, so panning cannot
paint across the comparison divider. The regression inspects actual canvas paint clips in
both pan directions; it fails when the old whole-canvas clip is restored. Comparison views
now pan with the wheel (Shift for horizontal motion), support middle-button drag, and anchor
modifier-wheel/pinch zoom to the pointer, including zoom limits. The panned browser capture
was inspected visually. These display gestures do not change project state.

This closes the reported pane bleed and the pointer-anchor issue. Region rendering, full
reference gesture acceptance, rulers, settling overlays and playback remain open as listed above.

Version 0.14.3 passed formatting, native/wasm Clippy, all 452 Rust tests, workspace build,
dependency policy, transport regressions and the expanded desktop browser workflow (including
pan/Fit, reload recovery and simulated cuts). No hardware or Safari acceptance is claimed.
