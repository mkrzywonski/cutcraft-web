# M4: the desktop layout — GLM's branch

**In progress; not an M4 completion or merge approval.** Based on M3 `b9ea099`. The reference is
CutCraft 1.0.5 (`39654a9`), with the design-parity contract (`codex/design-parity`, `ffc8821`–)
as the authority for what the desktop draws. M5 is not started and must not be while gates here
stay open.

## Where this branch is going

Claude's comparison review (2026-10-29) asked for the typed shared boundary to land before the M4
gates close. This branch builds that first, as its own slice: everything the desktop draws and
sends is typed in `cutcraft-web-protocol` from the server through to the window, so a server-side
shape change that misses the protocol bump cannot degrade silently into the browser. The desktop
window itself (the egui/wasm crate) comes after, against the typed snapshot rather than around
anonymous JSON.

## Landed

- **0.14.0 — the typed desktop view (protocol 6).** The session snapshot carries the issue/info
  cards, the rows with their prepared items, the sheet's settings, the material and mat lists in
  the registry's own field names, per-project units and snap, per-person theme, the panel
  arrangement, history with origins, and the shared screen. Geometry carries the plan's
  operations, diagnostics, piece and mat rulers. `settings.units`/`settings.snap` save with the
  project (spec §8); `settings.theme` is new. Export is split best-effort (Save) from validated
  (`cut.copy`), and neither names the worker's local source path. The worker applies `project.update`
  from an uploaded file and re-prepares on font changes. 447 tests, fmt, clippy
  (native, `-D warnings`), `cargo deny`, `scripts/design-check.sh` (protocol 6) and
  `scripts/browser-check.sh` pass.

## Open gates

- [ ] The desktop window: `crates/ui` (egui to wasm), CutCraft's chrome and panels drawn from the
      typed snapshot, served at `/desktop` behind the account check (§9.2).
- [ ] The client session model in Rust: typed `Session`/`Pending`/`Settle` with reconcile-by-key
      and reload checkpoints (the class of bug that failed Grok's first attempt), unit-tested.
- [ ] Desktop browser e2e: Save-As on first Save, view navigation, drag resizing, test cuts then
      a design cut, reload at both held points (the §13 row's currency).
- [ ] Fonts licence check for the egui build (no default fonts; Source Sans 3 under OFL-1.1 as
      grok/m4-parity did, or equivalent) and `cargo deny` over the wasm target.
- [ ] The cut screen and Pause/Stop from the desktop; the compact layout unchanged in behaviour.
- [ ] Version discipline kept: every commit that changes the program bumps and documents.

## Coordination

- `codex/m4` is the trunk of record per the comparison review; this branch's typed boundary is
  offered for folding there rather than competing with it.
- Grok's parity tree holds the typed client session model; if it lands on `grok/m4-parity` first,
  this branch adopts it instead of writing a second one.
