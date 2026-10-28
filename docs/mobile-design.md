# M5 mobile view design guide

Status: **design proposal, 2026-10-28**, for review before M5 implementation.
This extends [the web specification](spec.md) and preserves [the full CutCraft contract](cutcraft-ui.md).
It proposes phone presentation, not fewer capabilities or another preparation engine.
Nothing here approves M4, bypasses [the M1–M3 findings](design-audit/m3-review.md), or claims mobile
acceptance has passed. Grok's M4 implementation remains separate.

The recommendation: **one useful workspace at a time**. In portrait, show either a full-width
preview or full-width Controls. Use labelled navigation and familiar cards from cameo-pi.
Keep every preparation, material, mat, file and cutting action reachable without rotating the phone.
Landscape is an optional better comparison view, never a prerequisite for completing a task.

[Interactive layout study](design-audit/m5-mobile-study.html) illustrates the proposed preview,
Controls, material fields, landscape comparison and cut controls. It uses authored schematic art,
not live state. It cannot connect to a cutter. The guide is authoritative over that limited sketch.

## 1. Reference review and what it establishes

Reviewed CutCraft **1.0.5, `39654a9465ecdef7478703ee586276304dbc29c6`**:

- Actual native side-by-side, overlay and mat screenshots from the earlier reference run.
- A fresh isolated native run at 1280 × 900 logical pixels: menu, Materials library and material
  editor inspected; source code checked for menus, all ten groups, mat editor and cut controls.
- `apps/desktop/src/main.rs`, `chrome.rs`, `choose.rs`, `prepare.rs`, `preview.rs`, `cutting.rs`
  and `workspace.rs` provide the detailed control inventory. The command registry's menu metadata
  is not the literal UI menu; the actual native menus are the reference.

Reviewed cameo-pi **0.23.0, `d60d963a24517df79ec968a8d04b3cdb42b0af31`**:

- Actual served UI in headless Chromium at 390 × 844 CSS pixels, touch emulation, authored square,
  scratch data and only the practice cutter. Opened Cutters, Library, project detail, Adjust,
  loading and Settings. None of those pages overflowed horizontally at that size.
- `apps/server/web/{index.html,app.css,app.js}`: labelled bottom tabs, 48-pixel input/button
  minimums, 64-pixel primary actions, cards, full-width forms, folder picker and loading steps.
- No physical motion, no user/private artwork, and no changes to either reference checkout.
  Screenshot review is not a real-device usability or accessibility certification.

Useful cameo-pi patterns: persistent labelled navigation, plain cutter status, searchable/list
navigation, preview above a focused form, collapsible Settings and explicit load/Start steps.
Do **not** carry over its reduced preparation scope, external-desktop catalog sync, repeated
full-height administrative warnings, single long Adjust form, or combined “Save and cut” action.
Those conflict with the full web workflow or waste the phone's working area.

## 2. Classification of reference elements

“Suitable” means the element's form works on a portrait phone with normal touch sizing and
wrapping. “Not suitable as-is” means its interaction/layout must change, **not** that the feature
is excluded. No reference capability is removed by this table.

| Reference element | Portrait suitability | M5 treatment |
|---|---|---|
| Five-menu desktop bar | Not suitable as-is | Labelled project Actions sheet plus Settings/Help pages; inventory in §6. |
| Single desktop toolbar with every command | Not suitable as-is | Small contextual preview toolbar, visible Controls and separate Test Cut/Cut actions. Less-frequent actions in the labelled menu. |
| Icon-only Open/Save/Undo/Redo | Not suitable as-is | Labels or accessible names; Open/Save in project header/menu, Undo/Redo in contextual tools. Never depend on hover. |
| Current filename/path across the top | Not suitable as-is | Project name wraps to two lines; tap details for full name/library location, never a server path. |
| Unsaved, working, disconnected and error states | Suitable | Compact text/icon status with details on tap; no colour-only meaning. Blocking and active-cut states stay prominent. |
| Side-by-side artwork and lines | Not suitable as-is | Portrait Artwork/Cut lines toggle with identical framing, plus Overlay. Landscape Compare when both panes have useful width. |
| Artwork alone | Suitable | Full-width canvas with inspection zoom/pan and Fit view. |
| Cut lines alone | Suitable | Same framing as artwork; included-only lines and tappable row inspection. |
| Overlay | Suitable | Full-width source plus included cut lines; source/line visibility controls are explicit. |
| Whole stationary Cut Preview mat | Suitable for overview | Full-width mat with top-edge arrow, scale/rulers/material; numeric placement for precision. Never resize a design with an inspection pinch. |
| Fine mat ruler ticks/legend | Not suitable as-is | Thin ticks can be visual; labelled major ticks and dimensions remain readable. Legend expands on tap and never covers a selected handle. |
| Tiny design/media/test-spot handles | Not suitable as-is | Enlarged touch hit areas, explicit target selection and numeric alternative. See §5. |
| Hover highlights / right-click actions | Not suitable as-is | Tap Inspect/Show me and labelled contextual actions. Long press is optional, never required. |
| Right panel beside artwork | Not suitable as-is | Controls button opens a full-height page/sheet, with Back to preview preserving view/framing. |
| Ten panel group cards | Suitable individually | Full-width summary rows with expandable bodies; same groups, values and commands in all preview modes. |
| Group title/count/attention | Suitable | Large tap row; unresolved counts remain visible when closed. |
| Drag-to-reorder / tiny lock icons | Not suitable as-is | Arrange controls page with Move up/down and Pin controls; optional drag grip. Retain shared order/locks. |
| Multiple fixed locked groups above scrolling panel | Not suitable as-is | Pinned shortcut chips lead to groups; never pin entire long forms over the canvas. Desktop lock state remains meaningful. |
| Issue question with allowed choices | Suitable | Full-width card, plain explanation, separate Show me, vertically stacked choice buttons as needed. |
| Info notes / Run again / Clear answers | Suitable | Same content and actions in Controls; distinguish Info from blockers. |
| Width/height/proportions | Suitable | Stacked labelled fields, linked indicator, Original size; preserve fractions/units. |
| Colour/layer rows | Suitable | Full-width selectable/checkable rows with swatch, name and counts; separate Inspect action. |
| Material preset chooser | Suitable | Searchable full-height picker; wrap complete names, show material summary. |
| DragValue/spinner settings | Not suitable as-is | Labelled value field and large −/+ buttons; no dragging numbers to change force. |
| Mirror, weed box, weed lines, marks toggles | Suitable | Whole labelled row is tappable; nested options appear below. |
| Material Save / Save As | Suitable | Explicit “Save material” / “Save material as…” to distinguish project Save. No automatic preset save. |
| Advanced offset controls | Suitable | Collapsed Advanced section; all choices still available. |
| Mat selector / mat diagram | Suitable | Searchable list and full-width diagram with labelled dimension fields below. |
| Media size/origin and placement/angle | Suitable | Full-width fields, nudge arrows, Rotate 90°, Centre and Move to corner. |
| Playback slider and travel legend | Suitable after touch sizing | Large labelled slider and Play/Pause/step controls; estimated motion, not actual telemetry. |
| Material/mat library desktop window | Not suitable as-is | Full-height list route, search, visible selected-item action area. Tap selects; Edit is separate. |
| Floating/nested modal editors | Not suitable as-is | Full-height editor over its parent route; visible Save/Cancel, scrolling form, keyboard-safe actions. |
| Native Open/Save dialogs | Not suitable as-is | Browser picker and owned library picker. First Save asks folder/name; Downloads separate. |
| Cut/Test Cut screen with wide centre plus sidebar | Not suitable as-is | One-column review/load/status screen; preview and instructions above a persistent action area. |
| Loading checklist / blade/material notes | Suitable | Plain steps with full names and small mat diagram; Start stays distinct from navigation. |
| Pause/Resume and Stop | Suitable after enlargement | Visible active-execution bar; Stop immediate, never in overflow or behind confirmation. |
| Scenario selector | Suitable only for simulation | Clearly “Practice cutter scenario”; no production tool setting with this name. |
| Queue/history/list cards | Suitable | Extend cameo-pi; stable rows, visible actor/status, safe release/reorder interactions. |
| Troubleshooting, help, About | Suitable | Full-width reading pages, clickable links and Back; text size supported. |
| AI setup, named keys, account/backup forms | Suitable after reflow | Settings sections, wrapped text, Copy buttons, correct permissions; no secrets in URLs. |
| Keyboard shortcuts | Not sufficient on their own | Optional external-keyboard convenience; every action has touch access. |

## 3. Phone navigation and page structure

**M5-01 — Top-level destinations:** extend cameo-pi's bottom tabs to **Design · Library · Cutters ·
Settings**. Show labels with icons; no more than four destinations. Help lives in Settings and
has a shortcut in project Actions/issue cards. No sign-in/account controls hidden behind a
non-working desktop screen. Empty Design offers Open design and Open project.

**M5-02 — Design header:** project name, Save (state-aware), and labelled Actions. Below it, a
single compact readiness row: “2 issues”, “Ready to cut”, “Preparing…” or an actionable failure.
Tap the issue count to open Issues. A disabled Cut never doubles as “fix the issues”; its reason
is visible separately. Nonblocking backup/setup reminders live in Settings with a badge/summary,
not repeated tall banners above every canvas. Connection loss remains visible.

**M5-03 — Preview page:** a View control selects **Artwork · Cut lines · Overlay · Cut Preview**.
A separate **Controls** button opens the ten groups. Undo/Redo and view-specific tools are grouped
below the canvas or in Tools. The contextual action row contains **Test Cut** to the left of **Cut**,
matching the reference's two actions. Project Actions → Cut provides the menu equivalent; do not
add test links inside material cards, library entries, issue guidance or the loading checklist.

**M5-04 — Controls:** use a full-height route presented as a sheet/page in portrait, not a shallow
bottom drawer. This is the right panel's mobile presentation. Header: Back to preview, Controls,
Arrange. Keep the originating view, pan/zoom, selection and scroll position. Show a small optional
preview thumbnail only where it helps a visual setting; tap it to return to the full preview.
Avoid nested independent scrolling regions. Browser Back closes the current editor/sheet before
leaving Design. Swiping is optional; every transition has a button.

For a short control such as selecting a view, a compact bottom sheet is appropriate. For ten
groups, a material editor or a keyboard-driven form, use full height. On a sufficiently wide
compact landscape layout, a docked Controls drawer may replace the full page only if the canvas
still has at least 420 CSS pixels and all fields retain their minimum targets; otherwise keep the
full-height page. Closing a presentation sheet never discards accepted edits.

**M5-05 — Scope:** preview mode, phone route, folds, camera/zoom and sheet visibility are local to
the browser. Phone navigation never calls desktop `view.show`. Project settings and decisions
remain shared; another device sees their accepted effects. Panel order/locks and font/theme follow
their specified scopes. There is no phone-only project, material library or cutting authority.

## 4. Portrait, landscape and wide-window rules

**M5-06 — Portrait comparison:** Artwork and Cut lines render the same physical region and scale.
Switching toggles the representation, not the framing; Overlay uses that same transform. Labels
always say which representation is shown. Show me returns to the correct view and highlights the
object/row/issue, with Back to issue. No swipe gesture that can be confused with panning is required.

**M5-07 — Landscape comparison:** offer Compare side by side when the available canvas can give
both panes at least 300 CSS pixels after gutters and controls. Keep pan/zoom linked and source and
lines labelled. If the space is insufficient, retain the single-view toggle and Overlay. Rotation
is optional; do not lock orientation or show a blocking “rotate your phone” message. Rotating
back preserves region, selection and unsent field text; it does not change the project.

These are proposed layout thresholds to validate, not device-model detection. At 844 × 390, use
compact landscape chrome and no side panel during comparison. A short height needs less chrome,
not two miniature canvases under repeated title/status rows. All commands remain available through
buttons/sheets even when their shortcut leaves the toolbar.

**M5-08 — Layout selection:** retain developer decision #118: M4's desktop View menu is Side by
side, Overlay, Cut Preview; “Phone layout” is not a desktop menu item. A wide viewport opens the
desktop UI; a stored compact preference must not override width on a fresh wide load. Use the
spec's normal desktop threshold (1024 CSS pixels, or 768 with a fine pointer) unless amended;
phone landscape below that stays compact. Do not store a global “always compact” flag.

For an already open page crossing a layout threshold, offer the other layout at an idle point;
never replace the editor while typing, reviewing a cut, or handling an uncertain request. Keyboard
appearance changes the visual viewport height, not the layout selection. Rotation within compact
only reflows it. Test a 1024-wide tablet and a narrow desktop window explicitly. A phone is not
inferred from user-agent text.

## 5. Canvas, touch and precision

**M5-09 — Inspection is not resizing:** pinch/pan changes inspection framing only in Artwork,
Cut lines and Overlay. Fit view resets framing. The stationary whole-mat rule stays in Cut Preview:
no pinch gesture changes actual design/media dimensions, and no implicit mat pan/zoom is added by
this guide. “Fit design to material/mat”, “Original size” and numeric width are explicit project
edits, visibly labelled and undoable. Disable resize/Fit-design controls on Cut/Test Cut execution
screens, matching desktop. A future mat magnifier would need a separate agreed design.

**M5-10 — Select the manipulation target:** Cut Preview Tools provides **Design · Media · Test
spot**. Highlight only that target's handles; show its dimensions and relevant numeric controls.
A touch target may extend beyond a drawn handle without altering geometry. Do not guess whether
an overlapping finger meant design resize or media resize. Test-spot manipulation exists only as
placement/visibility, never another Test Cut entry point.

**M5-11 — Precision alternative:** every drag has numeric fields or step controls. Provide x/y,
width/height, angle, snap step, nudge, Rotate 90°, Centre on material, Move to corner and “Place test
spot automatically” as applicable. Keep units visible. Preserve unit suffixes and fractions
(including mixed numbers); use a text-capable field or keyboard switch rather than a numeric-only
keyboard that cannot enter a slash/unit. Local typing is not overwritten by live updates; commit
on Done/Enter/blur as specified, showing validation failures inline.

**M5-12 — Gestures:** drag a selected handle/object to edit, release to submit one command and one
Undo step. Touch scrolling outside the active canvas remains normal; do not disable browser zoom
for the whole document. Pointer cancellation, a second finger, orientation change or leaving the
page before release cancels the unsubmitted drag rather than sending partial edits. Stale drops
refresh and explain; no silent overwrite. Numeric controls remain available for motor/accessibility
needs, keyboard and Pencil. No hover-only, right-click-only or long-press-only command.

## 6. Complete action mapping

| Desktop action family | Phone route and preservation |
|---|---|
| Open / recent / uploaded designs / library project | Design → Actions → Open, Recent; Library → project → Open in Design. Browser file picker/drop where supported, same owned-upload handler. Opening replaces shared design only under unsaved-work policy. |
| Save / Save As / Download | Header Save; Actions → Save As / Download. Full-height folder/name picker on first Save. Conflict offers Reload or Save As. Cancel writes nothing. Distinguish project Save from material Save. |
| Update from Design / Close | Actions → Update from Design → pick newer upload, preview result/differences; Actions → Close design. No invented device-file watcher or Quit service action. |
| Undo / Redo / journal | Design Tools has Undo and Redo with latest action label; Actions → Activity for authorised origin-labelled history. A compact icon still has an accessible name. |
| Three desktop views | Four portrait representations in §4; landscape Compare. These supply side-by-side's full inspection ability without requiring two panes in portrait. |
| View zoom / Fit / physical size | Inspection controls only in artwork comparison modes; explicit physical-size tools in Artwork size/Cut Preview, with distinct labels. |
| Units / font size / theme / panel reset | Project units in Controls; Settings → Appearance for font/theme; Controls → Arrange → Reset. Reset affects the documented order/locks/folds, not artwork. |
| Materials / Mats | Controls → Material/Mat → Manage; also Settings → Materials and Mats without an open design. Shared household catalogs, selectable rows, explicit editor actions. |
| Cut / Test Cut / Exit | Preview contextual toolbar and Actions → Cut menu equivalents. Navigation only, then separate review/loading/Start. Contextual Exit on the corresponding idle screen; no stray test entry links. |
| Cut pause/resume/Stop/reset | Active cutter action bar; completion screen's explicit Start over resets project completion only, never safety receipts. Stop has no confirmation. |
| Help: design guide, troubleshooting, AI setup, manual, About | Settings → Help, plus Actions → Help and contextual issue links. Bundled content works on LAN without internet; external links identified. |
| AI connection and keys | Settings → AI tools. Human-only named key administration, Copy endpoint/setup, safe wrapping, show-once secrets and revocation. |
| Library folders/search/sort/notes/move/delete/restore/export | Library pages extend cameo-pi. Actions target selected item; non-destructive selection first, explicit delete. No required hover/context-click. |
| Accounts/password/users/backups | Settings with existing roles, accessible login/expired-session flow. Backup controls follow web spec, never AI self-administration. |
| Cutter selection/queue/history/release | Cutters pages extend cameo-pi; Design keeps selected cutter summary. Readiness/action controls refer to the explicit chosen cutter and reviewed copy. |

The M5 command-parity inventory must list every user-facing registry command and its route or an
approved exception. This table is a design map, not a substitute for that generated acceptance
inventory. Full MCP capabilities remain M6; UI must not bypass the shared registry to get ahead.

## 7. The ten Controls groups

All ten are reachable from **every** phone preview representation. Preserve their names and shared
ordering. Closed groups show a concise value/attention summary; nothing disappears because the
canvas is Artwork rather than Cut Preview.

| Group | Mobile content and interaction |
|---|---|
| Issues | Number of questions **and blockers**, plain cards/allowed answers, Show me, Run again, Clear answers, related help. Display pending/preparing results honestly; never auto-answer on navigation. |
| Info | Change/diagnostic notes, affected objects, supported alternate choices and explanation. Do not turn Info into an approval step. |
| Artwork size | Width/height, linked proportions, original dimensions/scale, Original size. Units and fractional entry work; Save captures accepted dimensions. |
| What to cut | Colour/Layer selector; full-width rows with swatch/name/count/tick. Whole checkbox label target; separate Inspect, so inspecting never toggles cutting. Retain stroke-role choices and single-sheet model. |
| Material | Searchable preset; blade/force/speed/passes/overcut, Mirrored, manual blade instruction; modified label, Save material/Save material as; Advanced offset; Manage materials. No tested flag/wizard. |
| Mat | Profile selector, dimensions/ruler summary, full-width diagram; Manage mats. Distinguish roll planning from supported hardware cutting. |
| Media | Known piece on/off, width/height, x/y; “Adjust on preview” selects Media target; Clear piece is explicit. Backend enforces cut-on-material bounds. |
| Cut lines | Left/top, angle/90°, Centre/Move to corner, snap and step. “Adjust on preview” selects Design target without discarding values. |
| Cutting aids | Weeding box/margin, weed lines/invert/spacing, layering marks; optional legend/placement explanation. All generated geometry uses same plan and reservations. |
| Estimate | Dimensions, cut/travel length, duration with approximate label, warnings, playback/order, moves-between-cuts and test-area visibility. Display changes coherently with latest plan. |

**M5-13 — Arrange:** touch reordering has Up/Down alternatives. Shared locked groups become pinned
shortcuts at the top of Controls, with overflow in a labelled Pinned menu if needed; they do not
consume half the phone with fixed forms. Fold state is local to this browser. Reset and font-size
changes never lose values or hide critical issue counts. Pinning does not imply material testing.

**M5-14 — Editing:** side-panel settings apply through the normal shared command on commitment;
there is no extra “Apply everything” or “Save and cut”. Temporary material tuning follows Q1,
including reload/sign-out/restart. Selecting a preset replaces it; library Save remains explicit.
Library editors are different: local draft until Save, Cancel discards only that editor's draft.
Save failure retains entered values. An external conflicting edit cannot silently replace typing.

## 8. Libraries, dialogs and forms

**M5-15 — Management pages:** Materials/Mats use searchable bordered rows. A single tap selects;
it does not open the editor or apply the preset. Show selected name and visible **Edit, Duplicate,
Delete, Default** actions; wrap them to two rows if needed, not under More. New stays in the header.
A picker that is choosing a preset is visually distinct: selection there applies that preset.
Keep the distinction clear in titles (“Choose material” versus “Manage materials”).

Editors fill the viewport with a clear title, scrollable body and Save/Cancel actions in a safe
area-aware footer. Do not stack floating modal windows. An editor is modal to its parent but
browser Back behaves as Cancel/discard-confirmation when there are uncommitted fields; it cannot
silently save. Return to the same list selection/scroll. Delete has an explicit confirmation with
the affected name, remains visible, and does not mutate saved project snapshots.

Material form retains all fields including loading notes, roll suitability, mirror, default and
advanced offset/overcut. Mat form retains outer dimensions, sticky bounds, each ruler edge's units,
cut start corner, load mode/length and approximate flag, with a diagram. Use sections; no horizontal
form scrolling. Large diagrams are inspectable separately, but every measurement is editable as text.

**M5-16 — Keyboard:** use visual viewport/dynamic height and safe-area insets; the focused field,
its error and a way to commit/cancel stay reachable with the keyboard open. Do not cover fields
with the bottom navigation or an unrelated sticky action bar. Long names and translated/large text
wrap; no ellipsis concealing a material's identity. Preserve padding at least 16 CSS pixels around
forms, with 12–16 between sections. These are design targets to validate, not certification claims.

## 9. Cut and Test Cut on a phone

**M5-17 — Same actions, staged clearly:** preview toolbar Cut opens Cut; Test Cut opens Test Cut.
Neither queues/freezes/starts automatically. The screen's Cut/Cut again or Cut pattern reviews a
validated frozen copy; queue selection/loading and **Start** remain explicit. Main-art readiness
and test-pattern readiness are independent. Choose cutter before reviewing its setup; changing
cutter or relevant settings invalidates that review.

The screen uses one column: named cutter/status, project and frozen-setting summary, mat/pattern
preview, concise loading steps, then Start. Manual blade setting, material side/mirror, actual
piece placement and occupied test areas are visible before Start. Expand details when long;
never omit them to fit. No test certification questionnaire, automatic force change or preset save.

**M5-18 — During execution:** reserve a persistent action area above any bottom navigation with
**Pause/Resume** and a large, labelled **Stop**. Stop is enabled during preparation delays and
reconnection attempts whenever a stop request can be submitted; network loss says “Stop not
confirmed” rather than pretending motion ended. Pausing is distinct from Paused. Progress is an
estimate with text and legend, not colour alone or fabricated telemetry. The active bar names its
cutter when several exist.

Exit is available before Start or after a terminal result; disabled during motion/uncertain
execution. Viewing Library/Settings may keep a persistent active-cut strip with Return and Stop;
it must not dismiss the service's execution or leave Stop behind an editor. Switching ordinary
Design views cannot silently abandon an uncompleted review. Browser close/reload detaches the
viewer, never starts/restarts/cancels a physical cut by itself.

**M5-19 — Repeat tests:** after a test, offer Try another pattern and Done in the existing test
workspace, with the next area shown. These are workflow continuation controls, not new stray
entry links. The user can adjust force/settings and review a new copy. Same-person retained-load
rules and invalidations in Q2 apply. Earlier spots remain occupied even when hidden. Changing
users, disconnecting, unloading, stopping or losing physical-state certainty requires fresh
loading. No phone-only exception to that authority. Ordinary preset Save remains a separate action.

## 10. Accessibility and visual treatment

**M5-20:** keep cameo-pi's calm cards, readable text and labelled controls; align colours/icons
with the new product. Proposed minimum target is 48 × 48 CSS pixels for interactive controls,
with 8-pixel separation where accidental taps matter; active Stop preferably 64 pixels high.
Body text starts around 16–17 CSS pixels and honours the app's text preference and browser text
scaling. Do not shrink type to make a desktop layout fit. Preserve 10–24 point preference semantics
and validate their browser mapping; minimum hit areas remain independent of chosen font size.

Use semantic headings, labelled form controls, button states, focus return and polite live status
announcements. Do not announce entire page/canvas on every progress tick. A modal traps focus
appropriately but not outside its visible controls. Swatches have text names; errors are linked
to inputs; dangerous actions use words, not only red. Reduced motion avoids sliding animations;
Controls still works as an immediate page transition. No gesture-only path. Dark and light modes
retain visible white artwork, selection outlines, mat boundaries and cut-state differentiation.

## 11. M5 implementation slices and dependencies

1. **Shell and navigation:** authenticated compact routes, four tabs, local preview state,
   keyboard/safe-area handling, complete read-only contract. Reuse cameo-pi cards/list/picker patterns.
2. **Preparation:** all four portrait representations, optional landscape Compare, Issues/Info
   choices and row selection, coherent source/line geometry, Show me/back, shared Undo/Redo.
3. **Settings and placement:** all ten groups, material/mat pickers and editors, project/media/test
   manipulation with numeric alternatives, scoped preferences and explicit saves.
4. **Files/library/help:** all formats, first Save/conflict/reopen/update/font upload, activity,
   library management, help/AI/account controls. No combined Save-and-cut shortcut.
5. **Cutting:** shared frozen handoff, loading and Start, retained-load test iteration, progress,
   Pause/Stop/reconnect/terminal states and persistent active-cutter strip.
6. **Parity and real-device acceptance:** finish tests below; do not mark M5 complete for a styled
   subset or a mobile screenshot alone.

Foundation dependencies are explicit in the [M1–M3 audit](design-audit/m3-review.md): safe complete
Issues/Info projection, full imports, Save unfinished projects, session Undo, project units/snap,
one catalog, owned update/fonts/drafts, and unified test placement/copy replacement/actor fencing.
M4 fixes may supply these; M5 consumes the same contracts. Do not work around missing fields with
invented client state, duplicated planning, direct filesystem access or weaker Start checks.

## 12. Acceptance checklist

Run real served compact-client interactions, not just mocked command calls. Store exact code,
browser, viewport and fixture versions. Use authored examples and simulation; physical acceptance
requires the developer present. Test all F01–F22/W01–W24 cases that apply, plus:

| Case | Required evidence |
|---|---|
| MP01 — Size/orientation | 360×740, 390×844, 430×932 portrait; 740×360 and 844×390 landscape; 768-wide tablet and 1024-wide boundary. No horizontal document overflow, inaccessible actions or forced orientation. |
| MP02 — Full capability inventory | Every desktop action maps to a compact route or approved exception. All ten groups reachable from all views; no missing file/material/mat/help operation. |
| MP03 — Comparison | Artwork/lines/overlay share exact region/scale before/after Show me, resize and rotation; landscape panes match. No stale source image or excluded-row ghost. |
| MP04 — Controls round trip | Open/close Controls and editors with Back/button; restore preview framing/focus/scroll; no accidental project change or field loss. |
| MP05 — Touch precision | Tiny/overlapping handles, cancellation, two fingers, nudge/numeric alternatives, fractions/units; one successful drop equals one edit/Undo. Mat overview never silently resizes artwork. |
| MP06 — Forms | Keyboard open, longest preset/name, all material/mat fields, 24-point preference and 200% browser text size; Save/Cancel/validation reachable. No nested clipped dialog or implicit save. |
| MP07 — Two-device work | Phone navigation leaves desktop screen alone; accepted edits update both. Origin-labelled Undo, independent folds, shared order/defaults/catalog, stale edits and lost replies behave correctly. |
| MP08 — Files | Start from an empty phone, open every supported format, answer issues, save unfinished/ready work, first Save cancellation, conflict, download, update source, font upload and reopen. |
| MP09 — Tuning | Preset select/modify/explicit Save/Save As; reload/logout/restart carry-over; test→tune→test→main with retained areas. No tested flag, questionnaire or Save-and-cut. |
| MP10 — Cutting | Navigation sends no motion; explicit handoff/Start; active Pause/Stop always reachable, keyboard/modal/page changes do not obscure Stop; uncertain outcomes never restart. All terminal and busy/offline states. |
| MP11 — User/cutter changes | Same person on another device may resume observing; other user/cutter/load requires revalidation. Tested actor fence, Stop race and disconnected load; no client relaxation. |
| MP12 — Access/navigation | Touch, keyboard, screen-reader labels/focus, reduced motion, dark/light, external links/Back, login expiry, failed network and font/zoom changes. Automatic snapshots do not steal focus. |
| MP13 — Wide-window rule | Fresh wide load opens desktop despite old compact storage. Rotating/keyboard resizing mid-edit preserves work; a portrait phone never needs desktop mode for a capability. |
| MP14 — Deployment | Served assets/version match; reload and second tab have unique command keys; no stale cached UI, private-path exposure or mismatched protocol. |

Chromium emulation is an early layout check only. Record separate hands-on iPhone Safari, Android
Chrome and iPad touch/Pencil checks; desktop automation does not establish thumb reach, virtual
keyboard, iOS file picker/download behaviour or real accessibility support.

## 9. Design-study validation

The standalone HTML study was exercised in headless Chromium at 360 × 740, 390 × 844
and 844 × 390 CSS pixels. Preview, all ten Controls groups and the active-cut illustration
rendered without horizontal overflow or JavaScript evaluation failures. Portrait and landscape
screenshots were visually inspected. Short landscape screens need compact headers and navigation
as well as sufficient pane width; this is an explicit M5 implementation check.

This is a limited layout illustration, not functional application code or an accessibility test.
Its schematic image, placeholder group contents and example progress do not establish rendering,
editing, device-control or full capability parity. Real-device gates above remain open.
