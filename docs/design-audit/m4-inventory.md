# M4 requirement inventory (first pass)

Status: **working punch list, not acceptance evidence.** Reviewed code: `codex/m4` at
`5765fe9` (0.15.1). Requirements: [the detailed contract](../cutcraft-ui.md), 71 IDs.
This is the start of the mapping that [acceptance §4.1](acceptance.md#4-evidence-required-to-close-m4)
asks for. It comes from reading the desktop client (`crates/ui`, `apps/server/web/desktop`) and
the browser scripts. It does not come from running each control, so every "Present" below
still needs its browser assertion before it counts. Nothing was run on hardware.

## Follow-up evidence

The tables below retain the first-pass baseline rather than claiming current acceptance.
Subsequent work is recorded in [M4 progress](../m4-progress.md):

- CUT-05/CUT-07: Stop retires retained loading; active-execution preview reload and occupied
  test-area overlays are implemented and simulator/browser tested (0.15.2–0.16.0).
- VIEW-03/MAT-06: reference outer mat, mixed per-edge rulers and printed grid zero are rendered
  and tested (0.17.0). Start/feed annotations and the rest of the view contract remain open.
- VIEW-01/UI-05: same-source images survive preparation changes while a new render loads;
  source/session/account changes discard private presentation state (0.17.1). All 22 authored
  fixture workflows pass. Region-at-zoom rendering remains open.
- Two baseline observations need qualification: the panel already caps locked groups at half
  its available height (`panels.rs::panel`); comparison primary-drag pans even with Space held.
  Neither observation alone establishes full panel or gesture acceptance.

## How to read the status

| Status | Meaning |
|---|---|
| **Browser-tested** | A control exists and `scripts/desktop-browser-check.mjs` or `desktop-transport-check.mjs` drives it through the real boundary. |
| **Present** | Control or output exists in the desktop client; no browser assertion yet. |
| **Partial** | Some of the requirement is reachable; the gaps are named. |
| **Missing** | No desktop control/output found for the named part. |
| **Backend** | Imported engine/planner/driver/service behaviour with Rust tests; the desktop only has to surface it. Browser exercise is still needed under ACCEPT-01. |
| **Later** | Belongs to M5/M6 or hardware acceptance; M4 needs only reachability. |

## Observations across the whole contract

1. **Before this branch, none of the 21 authored issue examples (`examples/issues/`) was used by
   any test or script.** `scripts/desktop-fixture-check.sh` now runs them all; see
   [Fixture run](#fixture-run-2026-11-05). It covers the report and the answer chain, not yet
   geometry, Show me, save-reopen or protocol parity.
2. **The desktop client is a new egui implementation, not the reference modules reused.**
   About 3,400 lines of `crates/ui` against about 8,400 in reference `apps/desktop/src`
   (excluding the control port/bridge). UI-01 says "reuse the reference egui UI modules". Either
   the gap closes or the developer approves the rewrite as an adaptation.
3. **There are 9 UI unit tests.** Most desktop behaviour has neither a unit test nor a browser assertion.
4. **The Cut Preview mat is drawn without rulers, its outer edge, the start corner, the feed direction
   or earlier test areas.** VIEW-03, MAT-06, CUT-05 and CUT-07 all depend on that drawing.
5. **Paired reference screenshots (ACCEPT-02, acceptance §4.2): none yet.** The browser check
   saves two unpaired desktop captures.

## Inventory

### 1. Interpretation and scope

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| BASE-01 | n/a | Interpretation rule. | — |
| BASE-02 | Backend | No node/text editing in UI. Source never overwritten (M2 server tests). | Browser assertion that Save/export/Update leave the uploaded bytes unchanged. |
| BASE-03 | Present | Single sheet; What to cut inclusion ticks. | Old multi-sheet project merge: no fixture/test found. |
| BASE-04 | Partial | Design guide, troubleshooting, AI help bundled via `include_str!`. | Help > Manual opens a GitHub URL (`toolbar.rs:59`, `dialogs.rs:83`); needs internet. |
| BASE-05 | Present | Issue/Info codes mapped to plain titles (`panels.rs` `problem_title`). | Fallback for unmapped codes; check against all fixtures. |

### 2. Opening and physical dimensions

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| OPEN-01 | Partial | File > Open… (picker), drop hint, Open from library…, Recent designs…. Picker tested with one SVG. | Drop, library and recent paths not browser-tested; check formats agree across picker/upload/MCP/help; AppleDouble/`__MACOSX`. |
| OPEN-02 | Partial | Failed import keeps previous design (server test `failed_import_preserves_previous_design_and_close_clears_it`). "Rendering artwork…" placeholder. | No progress/cancel control for open, trace, preparation, planning or region rendering. |
| OPEN-03 | Backend | Confirm size card ("Confirm {} × {} mm", "Check the size"). | F01 browser run. |
| OPEN-04 | Backend | "Picture size set" Info card. | Fixture with no DPI, small and oversized pictures. |
| OPEN-05 | Present | "Which page to cut", "Use the page appearance", "Use the editable layers". | F16 (two-page PDF) and AI cases in browser. |
| OPEN-06 | Present | "Trim the art to the page", "Use the artwork's area", "Art off the page left out". | F07 in browser. |
| OPEN-07 | Present | "Which units does this drawing use?" with mm/in/cm/ft/m. | DXF fixtures in browser; units invalidation on Update. |
| OPEN-08 | Backend | EPS cards ("Only the EPS preview could be read", "EPS read as one layer"). | EPS/PSD fixtures. |
| OPEN-09 | Partial | Linked-picture card exists in engine. | F14 in browser; bundle upload path from desktop. |
| OPEN-10 | Partial | "Missing font", "Use this substitute", "Use the letter shapes saved in the file". | **No font upload or per-person font management in the desktop** (progress doc gate). F02/F20. |

### 3. Tracing and preparation

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| PREP-01 | Partial | Run again, Clear answers, answer buttons via `prepare.decide`. | Per-step Advanced controls, keep script pieces separate (progress doc gate). Undo/Redo of answers not browser-tested. |
| PREP-02 | Present | Trace settings…: One colour/Several colours, Detail, Smoothness, Darkness threshold, Ignore white, "Apply trace and preview"; "Which frame to cut"; "Trace it anyway"; "Low-resolution picture". | GIF upload gap noted at M3 (`m3-conformance.md` PREP-02); no browser run. |
| PREP-03 | Backend | "This looks like a photo" card. | 20/21-colour and soft-edge fixtures. |
| PREP-04 | Present | "Text doesn't fit" / "Leave out the text that doesn't fit"; "Effect that can't be cut" / "Cut as solid". | F06, F03 in browser. |
| PREP-05 | Present | "Cut along line", "Cut around shape", "Part of the shape", "Cut separately", "Outlines on filled shapes". | F12, F15, F21; choice kept through resize. |
| PREP-06 | Backend | "Overlapping shapes joined", "Hidden parts trimmed". | Keep-text-separate control missing (see PREP-01). |
| PREP-07 | Present | "Background box?" Cut it / Don't cut; "Duplicate outlines removed". | F08 in browser. |
| PREP-08 | Present | Very small/thin pieces, Outlines that nearly repeat; Cut them anyway / Keep both. | F09–F11 in browser. |

### 4. Desktop shell, menus and common interaction

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| UI-01 | Partial | Menu, single-row icon toolbar, centre, right panel, bordered cards, vector icons (`chrome.rs`). | Rewrite rather than reuse (observation 2). No narrow-width "More tools". No paired screenshots. |
| UI-02 | Present | **File:** Open…, Open from library…, Recent designs…, Save, Save As…, Download project…, Update from Design…, Export ▸, Close design. **Edit:** Undo, Redo, History…, Materials…, Mats…, Preferences…, AI connection…. **View:** three views, context zoom/resize/fit, moves/test-spot toggles, inches/mm, font size and theme…, Reset panel layout, Phone layout. **Cut:** Cut Preview, Stop now, Cut…, Test Cut…, Cutters and queue…. **Help:** all five. | Only Help > Troubleshooting is browser-driven. "Open Recent" is an uploads list: confirm it also lists recent projects. |
| UI-03 | Partial | Toolbar with Undo/Redo enabled from `history` availability; Test Cut left of Cut; cut screens disable zoom (unit test). | Useful disabled reason on each control; Undo/Redo showing *whose* action; Exit/Stop rules (see CUT-03). |
| UI-04 | Partial | `document.title` = design name; menu bar name with "•" when unsaved; version in About. | Copy library location/reference; unavailable recent entries disabled with reason; empty-workspace hint exists. |
| UI-05 | Partial | Ctrl+O/S/Shift+S/Z/Y, view 1/2/3, Esc Stop (unit tests); text-field Undo kept (unit test); mm/cm/in/fractions/mixed numbers (unit test); wheel/Shift-wheel/pinch/middle-drag (browser-tested pan). | **Space-drag pan missing.** Touch/Pencil alternatives; Enter/Tab commit not browser-tested. |
| UI-06 | Partial | Text size 10–24 with reset; dark appearance (Preferences). | No check at large font/narrow widths; keyboard focus and contrast unverified; no cancel for open/upload waits. |
| UI-07 | Partial | Refused preview keeps connection (transport test); Sign in, Change password, Sign out, "Account recovery required". | Reason shown beside the action; distinct loading/failure/disconnected states (single `connected` flag today); 401 handling in browser. |
| UI-08 | Partial | EventSource live updates; stale/late responses dropped (transport tests); `view.show` shared. | Idempotency key reuse on uncertain retry not browser-tested; GLM's typed session (Zulip #129) not integrated. |

### 5. Views and the right panel

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| VIEW-01 | Partial | Side by side and Overlay share one transform; pane clipping and pointer zoom (unit + browser). | **Region-at-zoom re-rendering missing**: the whole artwork is one texture. **The old image is not kept while a new render loads**: `desktop.js` clears `model.picture` as soon as the art id changes, so the Artwork pane shows "Rendering artwork…" after an answer (fixture run, 11 of 22 files). |
| VIEW-02 | Partial | Excluded rows kept and peeked on hover; Show me toggles an orange highlight. | Show me does not zoom both panes to the affected geometry; keyboard/touch inspect action. |
| VIEW-03 | Partial | Stationary mat; sticky grid with 1-inch lines; material piece; Make design bigger/smaller in 10% steps; Fit design to mat. | **No rulers, outer edge, start corner or feed arrow drawn.** Automatic "Fit to the mat" on entering Cut Preview not found. |
| VIEW-04 | Partial | Design drag + corner resize (proportional, unit test); material corner resize (unit test); test-spot drag (unit test); drag-resize + Undo (browser). | Design box has one handle (se) only; **no material edge handles**; no dimensions drawn outside the design box or double-click-to-type on them; **no settling hold** after drop. |
| PANEL-01 | Present | All ten groups in all three views, default order. Contents largely as listed. | Estimate: loading edge and legend missing; check every listed field against reference. |
| PANEL-02 | Partial | "Issues · N" count; resolved cards. | Folded header amber/red with icon cue unverified; mat-only explanation text. |
| PANEL-03 | Partial | Fold, drag to reorder, lock; Reset; order/locks per person, folds per browser. | Insertion feedback; **locked groups capped at half the panel** missing; unknown-group tolerance untested. |
| PANEL-04 | Present | Colour/Layer toggle ("rows.group"). | Explicit near-colour merging capability; regroup inclusion rule untested. |

### 6. Materials and mats

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| MAT-01 | Present | Blade, Force, Speed, Passes, Mirrored, Overcut, Advanced offset method/value. Force edit browser-tested. | Profile limits shown/refused in browser. |
| MAT-02 | Partial | Session adjustment survives reload (browser-tested). Save material / Save material as. | Duplicate/invalid name keeps draft; sign-out and service restart in browser. |
| MAT-03 | Backend | No certification controls in UI. | — |
| MAT-04 | Backend | Presets imported with the library. | Browser check that all 14 presets and notes appear. |
| MAT-05 | Partial | Libraries with Search, New…, Edit…, Duplicate, Delete (with confirm), Use as default. | Live refresh from another session; failed-save retention; default listed first; Default checkbox in editor. |
| MAT-06 | Partial | Material editor: name, values, loading notes, roll suitability. Mat editor: outer size, sticky grid, ruler origin, per-edge ruler units, start corner, load length, approximate. | Mat **diagram** missing; rulers not drawn on the mat (observation 4). |
| MAT-07 | Present | `settings.new_design` in libraries. | No browser run. |

### 7. Planning, aids and geometry

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| PLAN-01 | Backend | Cut disabled unless ready; outdated copy needs "Prepare updated cut" (browser-tested). | — |
| PLAN-02 | Partial | "Show blade-up travel"; Off the sticky grid / Outside your material cards. | Red edge and smallest-safe-move fix action; roll switch-to-mat recovery. |
| PLAN-03 | Present | Weeding box + margin, Weed lines, Invert (stencil), Layering marks. | Planner tests are imported; no browser run. |
| PLAN-04 | Backend | Imported planner tests. | Preview/send parity by geometry. |
| PLAN-05 | Backend | Imported planner tests. | — |
| PLAN-06 | Partial | Estimated time; Play/Pause preview with order slider. | Legend; estimate breakdown; progress labelled as estimate on cut screen. |

### 8. Cutting and optional test cuts

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| CUT-01 | Browser-tested | Toolbar Cut navigates only; Start separate. | — |
| CUT-02 | Browser-tested | Test Cut screen, Cut pattern, Start, Try another pattern. | Done; test readiness independent of main design not browser-tested. |
| CUT-03 | Partial | Toolbar shows Exit on cut screens; Exit from test browser-tested; Exit/views disabled while busy. | Exit cancelling an unstarted review not asserted. |
| CUT-04 | Partial | Cutter picker before and on cut screens; "Simulated Cameo — no physical motion". | Loading-instruction content (material, side down, blade dial, preset notes) not checked; distinct busy/permission/disconnected explanations. |
| CUT-05 | Partial | Progress bar, "About N seconds remaining", Pause/Resume. | **Done black / remaining red / queued orange / blade marker on the mat missing**; legend. |
| CUT-06 | Partial | Stop through modal + Escape, durable `stopped` record (browser-tested). | Reload during active motion, unknown Start outcome, account change (progress doc gates). |
| CUT-07 | Partial | Next test spot drawn; reuse on one loaded mat browser-tested; occupied areas kept across reload. 0.15.2 (uncommitted) requires fresh loading after Stop. | **Earlier test areas not drawn solid and numbered**; dashed/labelled next spot; right-click hide/place; no-room guidance. |
| CUT-08 | Backend | Imported driver allowlist tests. | Hardware acceptance separate. |
| CUT-09 | Backend | Imported readiness/settling tests. | Hardware acceptance separate. |

### 9. Saving, updating, help and automation

| ID | Status | Desktop control / evidence | Gaps to close |
|---|---|---|---|
| SAVE-01 | Partial | First Save opens Save As (browser-tested); "This project was saved elsewhere" with Reload/Save As. | Conflict path and Save As cancel not browser-tested. |
| SAVE-02 | Backend | M2 project/save tests. | Reopen/edit/plan/simulator-cut of every released schema (acceptance §2). |
| SAVE-03 | Partial | File > Update from Design…. | Additions/removals display, retained exclusions, Undo back to previous snapshot: none browser-tested. |
| HELP-01 | Partial | All five entries open without a design; bundled guides rendered. | Manual needs internet (BASE-04); links from issue cards to the right guide. |
| HELP-02 | Missing | Exports: Cut plan, Cutter file (inspection only). | **No cut-log or problem-report download in the desktop.** |
| AUTO-01 | Later | Desktop goes through the shared registry. | M6. |
| AUTO-02 | Later | — | M6. |
| AUTO-03 | Partial | Edit > AI connection… opens settings keys. | Endpoint copy and setup text in the desktop; rest M6. |

### 10. Completion evidence

| ID | Status | Gaps to close |
|---|---|---|
| ACCEPT-01 | Missing | Browser assertions per requirement; this table is the index they should fill. |
| ACCEPT-02 | Missing | Paired screenshots against 1.0.5, all F/W cases, independent review. |

## Fixture run (2026-11-05)

`scripts/desktop-fixture-check.sh` (this branch) opens each file in `examples/issues/` and the
manual's mountain badge in the desktop, using real mouse input, against a practice service. For each
file it:

1. compares the initial report (cut-ready, questions, diagnostics, changes) with the 1.0.5
   observation in [fixtures.json](fixtures.json);
2. answers each question by clicking its visible Issues button, until none remain;
3. undoes every answer and expects the opened report back, then redoes and expects the answered one;
4. saves to the library through Save → Save As;
5. checks that the source artwork renders, and is not blanked while a newer render loads.

Result at `5765fe9`:

- **All 21 issue examples match the reference's initial report**, including the font cases
  02 and 20 with the bundled fonts.
- **Every question can be answered from its visible button.** 17 files end cut-ready. 12, 13,
  14 and 20 stay blocked by the expected blocking diagnostics, as in the reference.
- **Undo and Redo restore the exact report for every file. Save works for every file**,
  including the blocked ones.
- **One failure:** on 11 of 22 files the Artwork pane goes blank after an answer, while the new
  render loads (VIEW-01, above). Every render does arrive.

Seen in the screenshots, for review against the reference:

- The size question's button reads "Confirm 141.111 × 105.833 mm" (raw mm, three decimals) while
  the display unit is inches; the card text says 5.56 × 4.17 in, and Artwork size shows 4.17 × 3.47 in
  (the art, not the page). Check this against 1.0.5 (UI-05, BASE-05).
- The selected Colour/Layer toggle draws dark text on a dark fill and is nearly unreadable (UI-06 contrast).
- "1 pieces, 0 lines": singular/plural (BASE-05).

Not covered yet: the resulting geometry, Show me, save-reopen, other allowed answers (only the
preferred answer is clicked), the protocol/MCP route, and the acceptance-catalogue W cases.

## Tally

Counting the 68 IDs that carry M4 work (all except BASE-01, AUTO-01, AUTO-02):

| Status | Count |
|---|---|
| Browser-tested | 2 |
| Present | 16 |
| Partial | 33 |
| Backend | 14 |
| Missing | 3 |

These counts are rough. Very few rows are fully closed today: most "Present" and "Backend" rows need
only browser assertions, but most "Partial" rows still need product work.

## Suggested order for the remaining work

1. **The mat drawing:** rulers, outer edge, start corner/feed, numbered earlier test areas,
   cut-progress colouring. This closes parts of VIEW-03, MAT-06, CUT-05 and CUT-07 in one place.
2. **A fixture runner:** extend `desktop-browser-check.mjs` to open each `examples/issues` file
   and assert Issues/answers/geometry/Undo/save-reopen. This turns about 20 rows from Present/Backend
   into evidence and will surface real failures.
3. **Interaction gaps:** material edge handles, design dimensions + double-click, settling hold,
   Space pan, Show me zoom, locked-half cap.
4. **Fonts and Advanced preparation** (OPEN-10, PREP-01, PREP-06).
5. **HELP-02 logs/problem report**, Manual offline.
6. **Paired screenshots and independent review.**
