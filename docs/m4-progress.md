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
