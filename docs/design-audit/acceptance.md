# Port acceptance and regression catalogue

Status: **design contract**, not a list of passing web tests. Reference: CutCraft 1.0.5,
`39654a9465ecdef7478703ee586276304dbc29c6`. Requirements are in
[the detailed contract](../cutcraft-ui.md). No device was opened or moved for this audit.

## 1. Authored examples retained in this repository

[The issue examples](../../examples/issues/README.md) contain all 21 authored files from the
reference, byte-for-byte, with an updated README. [The mountain badge](../../examples/manual/mountain-badge.svg)
is the manual/workflow design. [The provenance manifest](fixtures.json) records original hashes
and compact initial1.0.5 observations without source paths, font paths or machine-specific logs.
They are original project fixtures, not derived from the private archive.

The reference CLI was built with `nix-shell --run 'cargo build -p cutcraft-cli --locked'` and all 21
issue examples were opened using `cutcraft-cli prepare FILE`, in isolated configuration, without
answers. Every command returned a preparation report. A successful process exit does **not** mean
cut-ready: only examples15 and 21 were immediately ready. Font outcomes depend on the controlled
font set; font/glyph tests must bundle/pin the required test fonts or assert their prerequisites.

For each fixture below, browser acceptance must verify: expected Issues/Info, Show me where there
are affected objects, choice/disabled-cut state, the actual resulting geometry, Undo/Redo,
save/reopen, and the same result through the protocol. An initial CLI report alone does not cover
that interaction chain. Use only allowed choices from the report; do not hardcode opaque IDs from
a previous import. Negative cases stay blocked even if a neighbouring issue is answered.

| Case | Fixture | Initial reference result and required exercise |
|---|---|---|
| F01 | `01-no-physical-size.svg` | `confirm_size`, not ready. Confirm vector size, then linked resize; contrast with automatic picture size. |
| F02 | `02-missing-font.svg` | `font_substitute`, not ready. Show family/replacement; explicit acceptance, or isolated font upload/reopen; preserve holes and words. |
| F03 | `03-gradient-fill.svg` | `appearance`, not ready. Choose each offered solid colour and exclusion; no silent flatten. |
| F04 | `04-semi-transparent.svg` | `appearance`, not ready. Cut solid/exclude, with source comparison and undoable result. |
| F05 | `05-embedded-picture.svg` | `exclude_picture`, not ready. Explicit exclusion permits retained vector art; no invented embedded tracing control. |
| F06 | `06-text-on-path-overflow.svg` | `text_overflow`, not ready. Explicitly accept only the fitting part; show affected text and editor guidance; do not claim omitted text survived. |
| F07 | `07-art-off-the-page.svg` | `fit_artwork`, not ready. Choose artwork bounds, retain physical scale, render in matching source coordinates. |
| F08 | `08-background-rectangle.svg` | `background`, not ready. Both Cut it and Don't cut; no default and no white-only exclusion. |
| F09 | `09-tiny-pieces.svg` | `small_pieces`, not ready. Cut them anyway is explicit and reversible; changed content invalidates the answer. |
| F10 | `10-thin-pieces.svg` | `thin_pieces`, not ready. Same acknowledgement/Show me/Undo requirements. |
| F11 | `11-near-duplicate-outlines.svg` | `near_duplicates`, not ready. Keep both; no automatic near-merge; distinguish exact deduplication. |
| F12 | `12-dashed-line.svg` | `stroke.unsupported` blocks. Explain centreline limitation; do not show it as cut-ready merely because a partial preview exists. |
| F13 | `13-empty.svg` | `preparation.empty` blocks; `svg.hidden_dropped` is Info. Nothing tickable/cuttable; no active Cut. |
| F14 | `14-linked-picture.svg` | `exclude_picture` plus blocking `link.fingerprint_pending`/`import.warning`. Excluding a picture cannot dismiss unrelated import integrity failures. Web ownership checks add their own safe refusal where necessary. |
| F15 | `15-outline-on-fill.svg` | Ready; `stroke.part_of_shape` Info. Default outer shape; Cut separately changes rows/geometry; Undo restores; colour/object override precedence. |
| F16 | `16-two-pages.pdf` | `page`, not ready. Page1 square/page2 octagon; dimensions and displayed source match chosen page. |
| F17 | `17-logo-to-trace.png` | **Also asks `trace_photo` in 1.0.5**, despite its flat-logo name, plus low-resolution Info and `picture.sized`. Antialiasing exceeds the conservative exact-colour limit. Trace anyway; never ask picture size. |
| F18 | `18-photo.jpg` | `trace_photo`, not ready; size/low-resolution Info. Guidance to original artwork/editor; Trace anyway is explicit, no promise of a clean silhouette. |
| F19 | `19-animated.gif` | `picture_frame`, not ready; first-frame trace/size Info while awaiting selection. Choose either composited frame; no size question. |
| F20 | `20-missing-letters.svg` | `font.missing_glyphs` blocks under reference font environment. Missing glyphs cannot be accepted as an ordinary family substitute. |
| F21 | `21-thick-line.svg` | Ready; `stroke.outlined` Info. Thick curve becomes a shape, hairline stays centreline; explicit alternate stroke role. |
| F22 | `manual/mountain-badge.svg` | Walkthrough artwork: answer its background question, include/exclude colours, compare source/cut/overlay, set material/mat/media/aids, save/reopen and simulator cut. Same page transform must align sun and mountains. |

The old examples README said F17–19 require size answers. That was superseded by `d2ad3fb`.
F17 illustrates why the colour threshold is a warning with an override, not a photo classifier.
The README and this table describe the inspected 1.0.5 results, not the earlier fixture authoring date.

## 2. Required workflow checks

Each workflow needs real UI interactions against the served wasm/compact client and real server
routes, plus assertions on the resulting shared state. Unit tests that call a controller directly
cannot substitute for browser coverage. All mutations use a scratch account/library and simulation.

| Case | Requirements | Pass conditions |
|---|---|---|
| W01 | UI-01–06, PANEL-01–03 | Inventory every menu/button/group/dialog against reference at 1280×800 and 1920×1080, font 14 and 24; no clipped action, duplicate Cut, lost group or inert button. Labels/icons/spacing/status colours have paired screenshot review, not only a pixel-diff threshold. |
| W02 | OPEN-01–10 | Upload/drop/recent/library routes agree on every supported format; case variations and invalid files; previous project survives errors/cancel/late results. Browser picker accepts what server can import, including `.vectorcraft`, GIF/BMP/PSD/PSB and `.cutcraft`. |
| W03 | OPEN-03–04, UI-05 | Authored one-inch cases for each format; SVG 96px/72pt/physical/viewBox/transforms; pictures valid/invalid/no DPI, below 1in, oversized portrait/landscape, explicit override; fractions/mm/cm/in and Enter/Tab; labels never round stored geometry. |
| W04 | OPEN-05–06, PREP-04 | AI editing data missing text/differing appearance, embedded font alternatives, wholly/partially off-page SVG, Trim alternative, EPS partial parse, empty document; every loss explained and source/cut coordinates agree. |
| W05 | PREP-01–08 | Recipe replay, every allowed choice/invalid choice, cancellation and stale source; Undo/Redo; holes/islands/clips/fill-stroke unions/script welds/multiple paints; near versus exact duplicates; no unrelated exclusion. |
| W06 | PREP-02–03, OPEN-08 |20/21 exact colours, flatness-only photo, transparent RGB/alpha variation/soft PSD edges, ≥90% light art on transparency, GIF frames, low resolution; same decoder for artwork and trace. Trace controls affect recipe/geometry, not only local UI. |
| W07 | BASE-03, PANEL-04, VIEW-02 | All rows initially included; group colour/layer preserving selection rules/material/placement; distinct spots; excluded rows disappear in cut pane/overlay, transient hover/keyboard/touch inspect only; same changes via a second client. |
| W08 | VIEW-01–02, UI-08 | Same physical source/line region after pan/zoom/Show me and page/DXF-unit changes. Delayed older image/geometry never displaces current output. PSD source displays. White art remains inspectable. |
| W09 | VIEW-03–04, PLAN-02 | Stationary mat; design resize vs media resize distinct; material/design/test drag, snapping and ruler origins; local smooth movement, one command/Undo/drop, no snap-back. Fit covers shrink/translate/no-op, with visible note and Undo; no fit in sender. |
| W10 | PANEL-01–03 | Ten groups in every view; reorder/lock/locked scrolling capped at half; folds persist locally, order/locks per person; closed Issues still signals questions/blockers; Reset restores both scopes. Old removed group does not corrupt preferences. |
| W11 | MAT-01–04,07 | Adjust all settings/mirror/offset; selecting preset reloads; Save changes only preset; Save As unique name selects copy; failures preserve draft; explicit defaults/new imports/saved projects follow precedence. Verify Q1 across reload, sign-out, restart, preset selection and explicit session close. |
| W12 | MAT-05–07 | Bordered selectable material/mat rows; one click never opens editor; explicit New/Edit/Duplicate/Delete/Default; modal save/cancel/errors; renamed/deleted defaults; diagram/rulers and custom origins; loaded projects don't mutate with library edits. |
| W13 | SAVE-01–02 | First Save asks folder/name and cancel creates nothing; later Save adds intended revision; Save As separate; conflict Reload/Save As; download is separate; denied write/HTTP error/refusal keeps dirty draft and explains problem. |
| W14 | SAVE-02–03 | Reopen with source/fonts/presets absent/changed; old supported schemas/multi-sheet migration/newer/corrupt schema; update source preserves unambiguous settings, reports ambiguous matches, invalidates stale answers; Undo returns prior snapshot. |
| W15 | PLAN-01–06 | Bounds after mirror/aids/offset/overcut/rounding/travel; software/machine mutual exclusion/FC0; thin holes/collapsed contours/duplicate cuts; material versus grid versus device limits; order/repeats/marks/invert/estimate and simulator virtual mat agree. |
| W16 | CUT-01–04 | Navigation alone has zero handoff/motion; stale/pending plan disables Cut; explicit freeze/review then loading then Start. Test has only toolbar/menu entry; contextual Exit cancels unstarted review preserving records; minus/Fit/plus disabled on both execution screens. |
| W17 | CUT-04–06 | Disconnected/connect/change cutter, awaiting setup, Ready, Sending, Draining, Pausing, Paused, Stopping, Done, Stopped, Interrupted and Refused all have useful views/actions; shared mat backdrop and estimated progress; Stop never confirms/restarts. |
| W18 | CUT-07, A08 | Test placement in valid gaps, collision/aids/material/no-room refusal, next/occupied spot parity, hide does not release area. Repeated test→design uses the explicitly approved load policy. No questionnaire, tested status, auto-force or auto-save. |
| W19 | UI-07–08, AUTO | Reload/new tab/account adoption/new worker do not reuse command keys; unchanged retries return original outcome; changed body refuses visibly; network loss looks up results; delayed requests cannot overwrite another device. Login/logout/expired login accessible in both layouts. |
| W20 | UI-08, web §§6–8 | Two devices one person receive live geometry/settings/view/history/cutter state; two people stay isolated; shared library save conflicts explicit; no focus/menu/scroll loss during updates; compact navigation does not move desktop screen. |
| W21 | CUT-06,08–09 | No media/false Ready/startup delay/slow full buffer/partial write/disconnect/power loss/stall/stale status/long-pause resume; Stop bypasses busy import and wins late Start; duplicate Start/unknown outcomes/restart/history deletion/update draining. Whole invalid stream sends zero bytes. |
| W22 | HELP-01–02, AUTO-02–03 | All five Help entries and issue links work without design; Troubleshooting links click; actual served build matches UI; redacted reports vs explicit detailed export; real-client capture truthfulness; named key setup/revocation and AI-origin history. |
| W23 | web §§9,22 | Compact 360×740/390×844 plus desktop breakpoints, wide-window desktop default/no surprise mid-edit switch, touch/Pencil/numeric alternatives, keyboard focus and text sizing, contrast/non-colour cues; same commands/outcomes. Record Safari/Firefox/iOS checks separately from Chromium. |
| W24 | web §§18–22, CUT-08 | Pi 5 isolation/memory/Stop timing, owned-file security, backup/migration restoration and simulator cut, versioned assets/headers/CSP/font licences, offline-LAN use; hardware air-pass/pen/scrap only with developer confirming and separate measured acceptance. |

## 3. Other authored fixtures and tests to retain or add

The 21 visible issues are a **small demonstration set**, not a complete importer suite. Preserve
all relevant original tests and fixtures from the self-contained backend import; do not delete a
regression just because its UI is not ported yet. Add browser fixtures when the existing test builds
a document in memory. Requirements for these families are mandatory even where the original prose
promised more than the final native controls exposed:

- Physical scale in AI/PDF/SVG/SVGZ/EPS/DXF/VectorCraft and raster DPI; pixel roots, transforms,
  percentages, native editing data, nonuniform viewBox and encoded Unicode.
- Fonts: bundled/uploaded/embedded/substitute/style/missing glyph, text paths/overflow,
  object-vs-character paint, nested instances, trustworthy vs unavailable original rendering.
- DXF: mm/in/unitless/newly declared units, curve families, inserted/recursive/huge blocks,
  layer visibility and lineweight, binary/nonplanar/partial omission, project round-trip.
- Paint/preparation: multiple appearances, brush/unsupported centreline failures, thick dashed
  segments, clips/nested masks/winding, holes/islands, script overlaps, duplicate reversal/start,
  explicit repeats, white foreground/background and source provenance after union/exclusion.
- Pictures: full decoder format matrix, PSD source rendering and light-on-transparent trace;
  opacity boundaries, exact-colour count and flatness separately, extreme decoded size/frame count.
- Project compatibility: authored examples for every supported released schema; captured settings,
  prepared text, source hash mismatch, atomic-save interruption, missing source, changed engine,
  preset snapshot, cut record migration and unknown future schema.
- UI regressions from the commit ledger: failed editor save, first Save cancellation, selected
  row vs edit, all groups/views, closed Issues attention, default rename/delete, source alignment,
  crop/fit differences, excluded-row ghosts, typed-field commit, no drag jump, copied source label,
  clickable guides, execution-screen sizing disabled and stale plan gating.

## 4. Evidence required to close M4

1. An inventory mapping **every requirement ID** to the desktop control, shared command or
   read-only output, and its test/evidence. Declared but unreachable commands fail acceptance.
2. Paired screenshots and interaction results with exact reference/web commits, same fixture,
   units/font/theme/view and viewport. Cover menus, all groups, both library/modal editors,
   Help, Cut Preview, loading, test cut, active cut, Stop and interruption. Snapshot equality
   is checked on normalized semantic state: ignore IDs/paths/timestamps but not geometry/settings.
3. All F/W cases relevant to desktop/import/preparation/cutting pass, with remaining compact/MCP/
   packaging/hardware gates named separately. A simulator run does not prove real cut quality.
4. Required Rust/wasm/browser/security/licence checks pass at the reviewed commit. The served
   wasm build matches the application version and is reproducible; no stale binary in source.
5. Verify the confirmed Q1/Q2 decisions; any additional proposed exclusion needs the developer's decision.
   No M5 start until M4's required controls and outcomes are complete. This proposal imports
   fixtures and defines tests; it does not claim those web tests already pass.
