# CutCraft behaviour carried into the web application

Status: **version 2 design proposal, 2026-10-28**. This is a normative part of
[the web specification](spec.md), not an implementation report. It supplies the detailed
contract previously missing from §9.2. The reference is **CutCraft 1.0.5,
39654a9465ecdef7478703ee586276304dbc29c6**, before station integration.

The [source audit](design-audit/source-map.md), [commit ledger](design-audit/history.md) and
[acceptance catalogue](design-audit/acceptance.md) explain where these requirements came from.
The [exceptions](layout-exceptions.md) distinguish adaptations, exclusions and deferred work.
No unchecked requirement is waived because a backend command exists or the page resembles it.

## 1. Interpretation and scope

**BASE-01.** Preserve the final workflow and its corrections, not every historical experiment.
Where the old prose contradicts later changes, the final deliberate change wins. A known defect
in 1.0.5 is not a parity requirement. Unimplemented original requirements remain requirements
unless explicitly deferred in the exceptions register; their implementation is not assumed.

**BASE-02.** CutCraft is a cutting tool, not a design editor. Open, make cut-ready, select,
arrange, preview and cut. Node/text editing belongs in Illustrator, VectorCraft or another editor.
Preparation choices and whole-design arrangement are in scope. No Adobe/Silhouette plugin is
needed. The original artwork is never overwritten, including by Save, export or automation.

**BASE-03.** One project has **one sheet**, with every colour/layer initially ticked, one
material and one tool. Untick colours, cut, change the selection, then cut another colour.
No automatic sheet-per-colour split, numbered-sheet UI, Move to sheet, or next-colour wizard.
Keep assembly positions across those cuts. Old multi-sheet projects merge rows preserving
inclusion, use the first sheet's settings, and discard incompatible old completion records.

**BASE-04.** No internet connection or cloud account is needed for preparation, help, library
saving or cutting on the home network. Open mode remains available. A browser does need the local
service: this is not an offline browser engine. External manual/source links may need internet;
bundled help must cover the workflow without it.

**BASE-05.** Plain language: design, project, sheet, material, mat, start corner, picture,
Make cut-ready, Cut preview. Do not put raw JSON reports or transport details in the inspector.
Technical diagnostics remain available to tools and explicit problem reports.

## 2. Opening and physical dimensions

**OPEN-01.** Browser picker, drop, uploaded-design list and recent/library entries all reach the
same import path. Recognised extensions are case-insensitive. Advertise exactly the supported
formats below; picker filters, upload validation, MCP and help must agree. Ignore AppleDouble
sidecars and `__MACOSX` entries. Multiple alternate formats are not automatically separate sheets.

| Kind | Required formats and behaviour |
|---|---|
| Vector | SVG/SVGZ, PDF, AI, EPS, ASCII DXF, `.vectorcraft`, legacy `.drawcraft` |
| Picture | PNG, JPG/JPEG, GIF, BMP, PSD/PSB |
| Project | `.cutcraft`, handled as a saved project, not a vector import |
| Unsupported | `.studio3`, DWG, binary DXF: specific recovery guidance, no guessed partial import |
| Deferred | AIT, EMF/WMF, Affinity, TIFF and WebP; do not advertise support until their acceptance gates pass |

**OPEN-02.** Import warnings, preview-only status, page/layer identity and source fingerprints
survive the adapter. A raster preview is never certified as editable vector geometry. Unknown
warnings and potentially omitted content require review or block cutting. Errors/cancellation
retain the previous complete project; late work cannot replace a newer open, close or edit.
Progress and cancellation cover opening, tracing, preparation, planning and region rendering.

**OPEN-03.** SVG uses 96 CSS pixels/inch; points use 72/inch. Honour physical dimensions,
viewBox, transforms, aspect behaviour, stroke scaling and compressed/encoded input without
altering source bytes. A viewBox-only/ambiguous vector size still needs confirmation. A picture
without DPI does **not**. Geometry-affecting CSS needs review; harmless paint classes or an
Illustrator DTD alone must not block. Never resolve external entities or fetch remote assets.

**OPEN-04.** Pictures use valid declared resolution, otherwise 96 DPI. Preserve proportions;
enlarge a picture whose longest edge is below 25.4 mm to 25.4 mm; fit an oversized starting
picture to the default mat with the weeding margin. Explicit user sizing overrides that default.
Report the calculated size in Info. Do not recreate “How big is the picture?” as a required issue.

**OPEN-05.** PDF/AI page choice is explicit for multiple pages. Check appearance against editing
data: where different, offer the supported PDF-appearance route and explain lost layer information.
Omitted Illustrator text must offer that recovery without declaring the partial editing copy ready.
Only diagnose “no PDF-compatible part” when the importer actually established that. A filename
containing “CUT” or “Outlines” proves nothing. Unsupported appearances cannot silently disappear.

**OPEN-06.** Preserve selected-page scale and report excluded off-page art. For single-page SVG,
art touching but crossing the page by more than 0.5 mm expands the page to that artwork's bounds;
wholly separate off-page art stays excluded. “Trim the art to the page” restores the original
page policy. When all art is off-page, offer using the artwork's area where supported. Explain
how to move excluded art onto the page in its editor, then upload an updated design. Do not
silently assume Illustrator art outside its editing-data recovery can be reconstructed.

**OPEN-07.** DXF imports ASCII model space, XY plane, at 1:1 before any explicit preview fit.
Honour `$INSUNITS`; absent/zero/unknown units ask mm/in/cm/ft/m and show the resulting dimensions
for each answer. Record the choice for preparation, source rendering and saving; newly declared
units on update invalidate an incompatible old answer. Unfilled paths are centrelines regardless
of plotting lineweight. Retain visible layer names; omit hidden/frozen/non-printing layers.
Expand blocks with transforms, including rotation/nonuniform scale; bound expansion to 100,000
objects. Preserve arcs, circles, ellipses, bulges and splines. Nonplanar/thick/omitted/unsupported
content cannot yield a silently partial ready cut. Paper layouts and DWG require a suitable export.

**OPEN-08.** EPS uses the pinned engine's PostScript reader, with Illustrator editing layers
where available. A partial parse is refused (`import.eps_partial`); empty input means nothing to
cut, not success. PSD/PSB use the merged image, explicitly not the layers. The same decoder must
serve source artwork and tracing, so a successfully traced PSD also displays correctly.

**OPEN-09.** Linked assets resolve only inside an explicitly owned upload bundle, never arbitrary
server/client paths or network URLs. Missing/unverified links give embed/outline/re-upload guidance.
No promise of new linked-resource support merely because a parser accepts a link. Keep included
assets and fingerprints in a saved snapshot. Hidden objects/guides/nonprinting/template layers
are ignored and reported; locked visible objects remain included.

**OPEN-10.** Fonts are checked before conversion: family, style, missing glyphs and import-time
substitutions, including nested symbols. Use trustworthy embedded glyph outlines when recoverable;
an embedded-font entry alone is insufficient. Offer the file's saved letter shapes only when that
route preserves all relevant fonts; otherwise name the substitute and require acceptance, or offer
font upload/editor outlining. Missing glyphs/failed extraction block rather than vanish. Show
original comparison only when it is reliable. Fingerprint accepted fonts; saved outlined geometry
must repeat without reinstalling them. Per-person font isolation is mandatory.

## 3. Tracing and preparation

**PREP-01.** Make cut-ready runs on open; Run again and Clear answers remain reachable. Keep an
immutable source plus a validated recipe. Answers replay from source, replace the earlier answer
to that question, and preserve sheet settings. Deterministic replay gives equivalent geometry and
report. Undo/Redo operate on the recipe/project, not arbitrary destructive engine edits. Rejected,
cancelled or failed choices keep the last complete result; stale choices explain what changed.
Provide the specified per-step Advanced controls, including keeping script pieces separate,
through the shared recipe/validation path. Unsupported conversions must remain explicit blockers;
a control must never imply that an unsupported operation succeeded.

**PREP-02.** Standalone pictures default to one-colour tracing; several-colour tracing produces
colour groups, not fabricated source layers. Provide Detail, Smoothness, darkness threshold and
Ignore white with preview and recipe persistence. Animated GIF asks for a composited frame,
not a size; retain bounded frame/pixel decoding. Low resolution at the requested cut size is
reported. An embedded picture in a vector design requires explicit Don't cut; in-place tracing
is deferred. A photo warning links to original artwork/editor tracing guidance and offers Trace
anyway, never an assurance that the trace is suitable.

**PREP-03.** Ask before tracing when there are **more than 20 exact RGB colours on opaque
pixels**, or the colour-flatness heuristic flags the picture. At 1.0.5 the exact count uses
alpha ≥250 (the later PSD fix supersedes the older spec's ≥128); alpha changes alone and hidden
RGB values do not add colours. The flatness test samples visible alpha ≥128, quantizes RGB to
4 bits/channel and flags less than 90% in the four commonest buckets. Keep these distinct tests.
A transparent image with at least 90% of its visible pixels lighter than the one-colour threshold
traces visible art in its average colour; say so in Info. This preserves white designs for dark
shirts and avoids tracing nothing or just shadows. Test 20/21 colours and soft transparent edges.

**PREP-04.** Expand instances/visibility/transforms first, then font-checked text; preserve text
object and character appearance stacking. Text overflowing a path asks before excluding the
part that does not fit, offering a trustworthy page appearance where available. Expand supported
effects/brushes to real shapes; anything unsupported has an explicit solid-colour/exclude choice
where meaningful, otherwise a blocker. No “convert everything” operation may silently lose art.

**PREP-05.** Stroke interpretation is ordered: named/chosen cut-line colour is centreline;
a filled shape's stroke is part of the shape by default, joined to the fill in the fill colour;
an unfilled effective stroke ≤0.1 mm is centreline; thicker strokes become shapes. Honour effective
width after transforms, every appearance, caps/joins and supported dash segments. Unsupported
dashed/brush/variable/arrowed centrelines remain actionable blockers. Per-object choices override
per-colour choices, which override the design-wide Outlines on filled shapes choice, which overrides
defaults. Show Part of the shape/Cut separately only when relevant. Keep choices through resize.

**PREP-06.** Apply nested clipping intersections with winding and holes preserved. A cut line
crossing a clip may be refused until safely supported; never close it along the clip boundary.
Combine same-object fill and outline before cross-object unions. Unite only filled regions of
the same layer, colour and operation, retaining source provenance and real holes. Touching script
letters weld; keep-text-separate control remains a specified capability. Split union results into
pieces with holes, and detect failed/empty/area-losing unions. Explicit centrelines stay independent.

**PREP-07.** Remove exact duplicate contours within the same operation settings, including reversed
or rotated-start duplicates. Do not remove intentional passes or another operation's cuts.
Near-duplicates are a question, not an automatic merge. Invisible unpainted paths are excluded.
A bottommost page-sized background box needs Cut it/Don't cut; there is no default, and white alone
never means background. Excluding a source recomputes its weld; excluding a welded piece excludes
the whole result, with the affected scope explained.

**PREP-08.** Tiny pieces, thin pieces and near-duplicate outlines become Issues requiring explicit
Cut them anyway/Keep both acknowledgement. Keep the reference detectors (area below 1 mm²;
thin-feature inset check; near contours by geometry), not a screen-pixel approximation. Features
that disappear at device resolution are separately reported and cannot become invisible cuts.
Known-shape gradients/patterns/transparency/effects allow a chosen solid colour or exclusion;
opacity masks with unrecoverable shape allow exclusion only. Nested questions wait for the parent
choice. Preserve provenance through every split, clip, trace and weld.

## 4. Desktop shell, menus and common interaction

**UI-01.** Reuse the reference egui UI modules behind the browser contract. Preserve the menu/
single-row graphical toolbar/centre/right-panel/status arrangement, typography hierarchy, padding,
vector icons, bordered cards and restrained status colours. No text-art group outlines or several
rows of plain text buttons. Narrow widths/larger text may shorten view labels and put secondary
controls in More tools; Cut/Test Cut/Stop stay obvious. Compare screenshots of the same fixture,
view, font size and theme; a superficial likeness does not establish behavioural parity.

**UI-02.** The following menu inventory is mandatory. New web facilities may be added without
removing these capabilities; browser substitutions are named explicitly here and in the exceptions.

| Menu | Entries and behaviour |
|---|---|
| File | Open, Open Recent, Save, Save As, Update from Design, Close. Add Library and Download project/cut lines. Quit is intentionally absent: use the browser's tab/window controls. |
| Edit | Undo, Redo, Materials, Mats, AI Connection. Web account/font/settings pages remain reachable too. |
| View | Side by side, Overlay, Cut Preview; context-sensitive Zoom In/Out/Fit or Make Design Bigger/Smaller/Fit Design to Mat; Reset Panel Layout; Interface font size (10–24 pt, reset 14); inches/mm; Show Moves Between Cuts; Show Test-Cut Spot. Add the layout override and theme choice. |
| Cut | Cut Preview, Test Cut, Cut, Stop. No redundant Cutter submenu. |
| Help | Troubleshooting, Manual, How to Save Designs for Cutting, Set Up AI Tools, About. |

**UI-03.** Toolbar: Open (and recent access), Save, Undo/Redo, three views, minus/Fit/plus,
Test Cut with miniature square-and-triangle immediately left of Cut. Contextual Exit/Stop rules
are in §8. Secondary materials/mats/units remain reachable. Menu, shortcut, toolbar and MCP use
the same action/availability rules, with a useful disabled reason. A control is not complete if
it paints but discards its response. Undo/Redo show whether history exists and whose action it was.

**UI-04.** Show the working project name (otherwise uploaded design name) in browser title and
menu/status area, plus unsaved state and actual served version/build. Copy the authorised library
location/reference, never a server filesystem path. A quiet empty-workspace hint remains; avoid
an unsolicited wizard. Recent entries are per-person owned IDs, with unavailable entries disabled
and a reason. No local pathname from another machine masquerades as an accessible recent file.

**UI-05.** Ctrl/Command conventions apply where the browser permits: Open, Save, Save As,
Undo/Redo, views 1/2/3, zoom/fit and Esc Stop while a cut is active. Browser-reserved tab/quit/new/
zoom shortcuts cannot be promised; always provide working menus/buttons. Do not steal typing
from fields. Wheel scrolls vertically, Shift+wheel horizontally; modifier-wheel/pinch zoom about
the pointer in comparison views; Space/middle drag pans. Touch/Pencil and numeric alternatives
must reach the same result. Enter and Tab commit typed dimensions/positions; intermediate typing
survives redraw/focus changes until the authoritative value actually changes. Length fields accept
explicit mm/cm/in and fractions, including mixed numbers, independent of display units. Default
display is inches with two decimals; stored geometry and browser parameters retain precise mm.
Mat x is across and y is feed from the cut start; ruler zero and assembly coordinates are distinct.

**UI-06.** Font-size changes apply immediately to menus, panels and dialogs, persist per person,
and do not clip controls. Dialogs have padding on every edge, readable titles, scrolling bodies
and visible actions at supported viewport sizes. Open/upload/save waits cannot freeze status or
Stop; cancel writes nothing. Surface blocked/failed picker actions with recovery, not silence.
Keyboard focus, non-colour status cues and readable contrast are required in both themes.

**UI-07.** Every response, including HTTP200 with a command refusal, is handled. Show the reason
beside the relevant action without discarding unsaved input. Network/401/storage errors cannot
look like an empty successful design. Desktop and compact login/logout/account recovery are
reachable before protected requests. Loading/failure/disconnected states are distinct.

**UI-08.** Server changes arrive live. New snapshot/geometry/artwork must be coherent and older
responses must never overwrite newer state. Distinguish project revision, session incarnation,
view/settings changes and cutter updates; updates without a geometry change still appear. IDs for
mutations are unique across tabs, reloads and later commands; an uncertain retry reuses exactly
its original key/body and looks up the result first. Reconnect never blindly retries Start.
Desktop screen changes use shared `view.show`; compact pages do not change the desktop screen.

## 5. Views and the right panel

**VIEW-01.** Side by side shows source artwork left and included cut lines right in the same
physical coordinate region at the same zoom. Overlay uses that exact transform for both layers,
including page offset/rotation/size; independently fitting each layer is wrong. Paths remain vectors;
source artwork re-renders off-thread for the visible region, preserving the old image while waiting.

**VIEW-02.** Excluded rows remain in What to cut, labelled not cut, but are absent from the cut
lines and Overlay. Hovering an excluded row temporarily shows faint outlines; keyboard selection
and compact touch provide an equivalent inspect action. Selecting/highlighting never toggles
inclusion. Show me zooms both comparison panes to affected source/piece geometry and outlines it
orange; pressing it again clears the highlight. The artwork view still shows the source artwork.

**VIEW-03.** Cut Preview shows the stationary whole physical mat/roll, rulers, outer edge,
sticky grid, start corner, feed direction, material, intended contours, aids and test spots.
Do not pan/zoom the desktop mat. Minus/plus resize the whole design in the reference 10% scale
steps; Fit sizes and places it inside the material or sticky grid with margins/tool clearance.
Opening Cut Preview shrinks oversized art or translates art that only reaches off, with a calm
note and one Undo labelled Fit to the mat. This visible, undoable arrangement action is the only
automatic fit here; the planner/sender must never silently clamp or shrink. Preserve import scale
before that action. On **Cut and Test Cut screens**, minus/Fit/plus, corresponding menus and
shortcuts are disabled even while idle (1.0.4).

**VIEW-04.** Design blue box: drag placement; linked corner resize of all colours; dimensions
outside its sides; double-click a dimension to type it with top-left fixed. Material amber box:
drag placement, corner/edge resize; opposite corner/edge stays fixed, edges change one dimension,
no crossed/negative boxes; resizing material does not resize art. Test spot is draggable separately.
One command/Undo per completed gesture, not a stream of durable edits. Retain dropped geometry
until the accepted plan arrives or the bounded settling wait ends; refusal restores server truth
and explains why. Snap uses ruler origin/units and snaps size steps as well as placement.

**PANEL-01.** Cutter and cut-readiness stay at the top. These **ten groups appear in all three
design views**, in this default order, with controls operating on the same state:

| Group | Required contents |
|---|---|
| Issues | Pending questions, blockers, resolved cards; counts; Show me; allowed answer buttons; Run again; Clear answers; linked design help; AI-origin marks |
| Info | Titled blue change/note cards, folded by default, with applicable alternatives and Show me |
| Artwork size | Linked W/H, current physical cut size, Original size |
| What to cut | Colour/Layer toggle, swatches/names, piece/line counts, inclusion checkboxes, selection/hover; Outlines on filled shapes when relevant |
| Material | Compact preset picker, blade/force/speed/passes/mirrored/overcut, Save/Save As/Materials, Advanced offset |
| Mat | Mat/roll picker, outer/sticky measurements, approximate flag, Edit mats |
| Media | Cutting a piece toggle, W/H and Left/Top on rulers, To the grid's corner; no declared piece means whole usable area |
| Cut lines | Left/Top, Turned angle and 90° action, Move to corner, Centre on material, Snap to grid with step |
| Cutting aids | Weeding box and margin, weed lines, Invert: keep the background, layering marks; Show test-cut spot, Place test cut automatically, Show moves between cuts |
| Estimate | Design size, required vinyl/material bounds, approximate duration including travel/repeats, loading edge and cut-order play/scrub |

**PANEL-02.** Mat-only interactions/playback explain that they appear in Cut Preview; their settings
remain reachable from all views. No separate View options group (merged in 1.0.2). Cards have clear
borders/padding. Issues use a warning count, short titles, readable detail and resolved tick/green;
a folded Issues header remains amber for questions/red for blockers with text/icon cues (1.0.3).

**PANEL-03.** Headers fold, drag to reorder with insertion feedback, and lock above the scrolling
part. Lock icon means scroll lock, not disabling fields. Locked groups occupy at most half the
panel and scroll internally beyond that. Dropping among locked groups locks the group. Reset
restores default order/locks/open states. Order/locks are per-person; folds are per-browser under
web §8, intentionally replacing the native single-window preference. Persist both across reloads;
unknown/removed groups must not break a saved arrangement.

**PANEL-04.** Colour is default grouping; suggest Layer only for meaningful named layers.
Distinct spot identities stay distinct even when visually similar. Explicit near-colour merging
remains a specified capability; never auto-merge by appearance. Regroup preserves material and
placement and keeps a new row unticked only if every contributor was unticked. Selection and
exclusion preserve provenance, welding scope and assembly coordinates.

## 6. Materials and mats

**MAT-01.** Inline settings: blade depth 1–10 (manual dial), force 1–33, speed 1–10, passes 1–3,
Mirrored, closed-contour overcut 0–2 mm, advanced offset method Off/software/machine and value.
Validate against the connected profile; never suggest that changing blade number moves its dial.
Choosing a preset loads its saved values, including mirror, and discards unsaved adjustments.
Changing settings invalidates the plan/reviewed start and cannot modify an executing copy.

**MAT-02.** Adjustments belong to the current person's design session, not browser-local state
or an automatically modified preset. Material Save updates the selected preset; Save As asks for
an unused valid name, saves/selects the copy. Failed/duplicate/invalid saves preserve draft and
library. Project Save captures sheet values without saving the preset. Unsaved values carry across
views and new designs unless an explicit new-design default supplies a replacement; reopening a
saved project uses its own settings. Library changes never mutate saved snapshots silently.
Temporary adjustments survive reloads, worker eviction, sign-out and service restarts. Selecting
a preset replaces them; explicitly closing the design session ends carry-over. Recovery is scoped
to the same authenticated person. Preset Save/Save As remains explicit (web §23 Q1).

**MAT-03.** No tested/provisional status, certification checkbox, automatic force adjustment,
result questionnaire, material-test workspace or test gating. Legacy status fields are read for
compatibility, ignored and omitted on saving. Retain the small optional test pattern only (§8).

**MAT-04.** Preserve both Oracal vinyl presets and the twelve grouped Siser presets from the
reference: seven HTV (EasyWeed group, Glitter, Holographic/Aurora, StripFlock Pro, EasySubli,
Brick600, Easy Puff Glitter), five EasyPSV (Brushed, Etch, Glitter group, Glow, Blackboard).
Preserve numeric values, grouped-product/loading notes, mirror defaults, roll suitability and
chart provenance at the source revision; do not invent updated manufacturer values. They are
starting settings, not certification. EasyWeed Adhesive, cardstock and sketch-pen user workflows
remain excluded/deferred. Built-in offset remains Off pending real blade calibration.

**MAT-05.** Material and mat catalogs are shared by the household (web §8); edits refresh
other sessions without changing saved project snapshots. Materials and Mats open tall, scrollable libraries with visually distinct bordered
rows and summaries. Single click **selects**, never opens an editor. Explicit New, Edit settings,
Duplicate, Delete and Make Default actions remain visible; Delete is not hidden under More.
Edit/New/Duplicate open padded modal editors. Search/filter, empty states and write errors are
visible. Editing and deletion do not modify existing project snapshots. Successful save closes
the draft; failed save retains it. Default is tagged/listed first; editor also has a Default
checkbox. Renaming follows default identity; deleting it has a safe documented fallback.

**MAT-06.** Material editor includes name, inline cutting values, iron-on/mirror, roll suitability,
loading notes and offset. Mat editor includes name, whole-mat W/H, sticky-grid margins, grid and
cut origins, ruler units per edge, load length/12×12 or 12×24 mode, approximate flag and diagram.
Built-ins include the reference mat profiles; custom measured mats must work. Display rulers
and pointer/numeric positions from the same transform. Physical mat edge, sticky grid, device
area and material piece are separate; a “12 inch” label is not proof of usable cutting width.

**MAT-07.** Explicit per-person new-design defaults win for new imports. Without a valid default,
use session material (else first built-in) and 12×12 mat. Saved projects retain their own copies.
Defaults are managed in the library/editor, not duplicate checkboxes under the right panel.

## 7. Planning, aids and geometry

**PLAN-01.** One immutable plan drives preview, tool-motion overlay, export and sender. It includes
input/profile identity, ordered operations in mm, transforms, compensation, bounds, settings,
estimate and diagnostics. Geometry/inclusion/placement/material/mirror/preparation/profile changes
invalidate it. Until a current valid replacement arrives, Cut is disabled. Validate again before
Start against the selected model and frozen copy. Unsupported/empty/invalid plans cannot start.

**PLAN-02.** Display intended cut contours separately from compensated tool motion and optional
blade-up travel. Bound all generated paths, aids, repeat passes, compensation lead-ins and rounded
machine moves against the appropriate device/mat/material areas. Known material must contain
blade-down paths. Off-sticky-grid warning shows the edge red and offers the smallest safe move;
outside-device/material failures block. Fix actions are explicit undoable edits, never sender
clamping. Mirror warnings remain advisory where appropriate; unsupported roll material has a
switch-to-mat recovery.

**PLAN-03.** Weeding box on by default with 6.35 mm margin, encloses included artwork/marks and
cuts after artwork; weed lines off by default, clipped to waste with clearance and regenerated
after edits. Invert: keep the background sends weed lines through discarded shapes instead.
Layering marks off by default: four 6 mm inward triangles with tips 3 mm from whole-design bounds,
on its centre lines. Preserve their assembly positions regardless of selected colours; transform
with the cut/mirror. Never cut through kept art/test reservations; explain unsupported mixed
mirror assemblies. No automatic nesting or independent piece packing.

**PLAN-04.** Resize/rotation/mirror precede flattening; maximum target deviation 0.025 mm before
quantization, nominal device step 0.05 mm accounted separately. Round absolute positions;
no accumulated error. Validate finite/positive geometry, topology, holes, degeneracy and collapsed
features; report rather than silently turn a filled piece into an open line. Test preview/send
parity by geometry/order/settings/repeats, not screenshots alone.

**PLAN-05.** Containment order cuts holes/nested islands before release contours; repeat each
contour before its enclosure releases it; deterministic travel ordering and generated waste cuts
last. Software and machine blade compensation are mutually exclusive. Off/software explicitly
send FC0; machine mode includes measured clearance. Overcut only closed blade contours, bounded
by contour length, never extend open lines. Pen validation disables blade features. Roll planning
minimizes feed reversals but is not a claim of monotonic feed or hardware acceptance.

**PLAN-06.** Estimates include cut speed, travel, turns, point/lift overhead, startup, passes and
return travel. Retain the calibrated 1.0.5 model and its uncertainty: 9.3 mm/s per speed step,
100 mm/s travel, 0.005 s/radian, 0.15 s/lift, 0.008 s/point, roughly 1 s startup after idle.
Nonzero machine compensation adds unmodelled motion; pen measurements do not prove blade quality.
Cut-order play/scrub and legend must match the same plan. Approximate time is never measured progress.

## 8. Cutting and optional test cuts

**CUT-01.** Toolbar/Menu Cut performs **navigation only**, disabled unless preparation and the
current plan are ready. It never prepares, enqueues or starts motion as a side effect of entering
the screen. The screen's explicit Cut/Cut again action freezes/reviews the selected copy through
the cutter service; then loading confirmation and Start are separate. This resolves the old web
§14 wording that conflated the toolbar Cut and the handoff action. No duplicate Cut navigation
button in the design panel.

**CUT-02.** Test Cut is entered only through the toolbar (left of Cut) or Cut menu. It uses the
shared square-and-triangle pattern/settings; no wizard or material certification. Its screen has
Cut pattern, loading/Start/Cancel, progress, Try another pattern/Done. Test navigation itself sends
no motion. Require an open design as 1.0.5 does; standalone material wizard is deliberately removed.
Test safety/readiness is validated independently of whether the main artwork can be cut.

**CUT-03.** Toolbar Cut becomes Exit on Cut; Test Cut becomes Exit on Test Cut. Exit returns to
Cut Preview, cancels any unstarted review and preserves completion/occupied-area records. Exit
is disabled during motion. Stop remains conspicuous and immediate while cutting; it is not an Exit
or confirmation dialog. Ordinary view navigation cannot abandon an unresolved execution state.

**CUT-04.** Loading instructions name material, size/placement, side down, manual blade dial,
mat/load mode and preset notes. Physical confirmation plus settled machine readiness are required;
status alone proves neither correct material nor blade. Cutter picker uses server-owned cutters,
not browser USB, and is present before and on cutting screens. Changing cutter retains a useful
preview but invalidates device-dependent review. Explain unsupported/busy/permission/disconnected
states distinctly. Development/practice simulation is clearly identified and never opens USB.

**CUT-05.** Cut screen fills the centre with the same mat/rulers/material as Cut Preview; controls
and loading steps sit at right. Estimated done black, remaining red, queued orange, approximate
blade marker, percentage/time left explicitly labelled estimates with a legend/text cues. Pause
requests a contour-boundary checkpoint and shows Pausing until reached; Resume only with same
connection/load/plan. Idle cut settings remain editable, invalidating reviewed plans; active plans
are immutable. Browser edits to a different preparation draft follow web frozen-copy rules.

**CUT-06.** Stop bypasses import/edit admission and stops ordinary submissions immediately, while
buffered motion may finish. Acknowledgement is not physical halt. Unknown/disconnected state
cannot claim blade-up/completed. Interruption requires checking cutter and fresh loading; no
mid-cut replay on reconnect, refresh, login or worker restart. Terminal states distinguish Done,
Stopped, Interrupted/Unknown and Refused/nothing sent. Cut again requires a new explicit start.
Keep durable completion/interruption and nondeletable safety receipts separate from deletable
history. Update draining, actor/queue/turn/generation/epoch rules remain web §14 requirements.

**CUT-07.** Test pattern area is 25.4 mm square, placed clear of actual artwork (including gaps
inside its bounding box), aids, material edges and prior tests. Preview and executor choose the
same spot; invalid drag is red/refused. The next spot is dashed/labelled; earlier areas are solid,
numbered, and retained even when their overlay is hidden. Automatic placement and right-click
hide/place actions remain. Hide never frees occupied space. No-room explains how to use fresh
material. Consecutive tests and the following design may reuse the loaded mat under the
cutter-owned context in web §23 Q2/A08. Every cut requires Start. Unload, disconnect, Stop,
changing users, uncertain motion, mismatched setup or unproven physical state requires fresh
loading; a recovered preparation draft never establishes loaded state.

**CUT-08.** Driver retains whole-stream allowlist validation before any transport access: correct
framing, known commands, valid arguments and bounded coordinates, including hand-built jobs.
A forbidden late command refuses the entire stream with nothing sent. Discovery alone does not
open/claim/query a device. Only validated original Cameo motion is enabled; no inferred orientation,
homing, firmware-writing or unverified setup commands. Mat jobs lift/return to(0,0) in the job's
frame, estimate that travel; roll jobs lift in place. Setup experiments withdrawn in 1.0 stay out.

**CUT-09.** Retain transient-ready/startup/slow-buffer regressions: an immediate Ready after FG is
not load evidence; start waits for continuous Ready 3 s, polls 0.5 s, two no-media replies refuse,
15 s without settling interrupts without sending. Completion requires drained/stable idle, not
last byte; reference hold is 1.5 s and startup/setup allowances must be respected. Bound lead by
estimated motion and bytes, including long straight moves, without replay on partial transfer.
A slow full buffer is not automatically a fault. Long pause/resume reintroduces startup allowance.
Physical stop time remains measured separately; no promise of “about a second” from the old manual.

## 9. Saving, updating, help and automation

**SAVE-01.** First Save opens Save As for a **library folder/name**; cancel creates no project or
revision. Later Save targets the selected library project with its base revision; conflicts offer
Reload/Save As without silent overwrite. Save As creates/selects a new project. Download is a
separate browser operation; it does not silently choose a library save destination. Show dirty,
pending, saved and failed states accurately. Opening/closing protects unsaved work through the
web draft policy; it is not permission to discard it silently.

**SAVE-02.** Versioned self-contained projects contain source/needed owned assets, fingerprints,
prepared geometry, decisions/accepted substitutions, inclusion, assembly coordinates, material/mat
snapshots, size, placement, mirror, aids, cut records and engine/planner versions. Repeat from the
snapshot without original source/fonts/current presets. Never silently reprepare a released saved
revision. Unknown/damaged/newer schemas fail clearly without touching the file or current project;
atomic saves retain a readable previous version. Migration is tested by reopening/editing/planning
and simulator cutting, not only listing a library entry.

**SAVE-03.** Update from Design explicitly uploads/replaces source. Retain sheet settings and
unambiguous row exclusions; clear/report unsafe per-object matches. Duplicate layer names/renamed
colours/changed geometry must not transfer exclusions to another object. Display additions/removals
and unresolved choices; retain prior snapshot until accepted; Undo can return to it. Invalidate
font/unit/appearance answers when affected. No automatic local-file watch in a browser.

**HELP-01.** All five Help entries work without a design. Bundled guides render readable headings,
lists, code and clickable links; references from issue cards open the relevant guide. Manual and
About point to the new project's maintained manual, not a soon-deleted repository. Troubleshooting
covers shallow/deep cuts, tearing, lifting, incomplete separation, mirror errors and no-cut, with
blade/force/speed/passes/overcut adjustments and physical blade/mat/load checks. No wizard is needed.

**HELP-02.** Keep bounded cut logs and problem-report access through authorised downloads instead
of Show in folder/Copy server path. Default report includes version/engine/platform/model/state,
plan reference and bounded error/command outcomes; omits secrets, source text, private paths,
artwork and raw coordinates. Explicit detailed exports may contain artwork/commands and say so.
Raw cutter export is inspection-only, never an upload-and-send escape hatch. Reports are not
uploaded to a support/cloud service automatically.

**AUTO-01.** Shared registry actions retain typed schemas, availability/disabled reasons, stable
IDs/codes, revision checking, history and origin-labelled journal. Both layouts and MCP use the
same validation. Atomic edit batches roll back and form one Undo; irreversible file/device actions
cannot claim rollback. No raw engine/shell path. AI may perform authorised cuts under the same
physical preconditions; it cannot grant itself keys or administer users/backups. A source-conversion
question remains for the person unless they authorise the agent to answer it.

**AUTO-02.** MCP keeps inspection, preparation, rows, settings, material/mat libraries, plans,
preview exports, history/journal, help and cutter operations, with owned identifiers replacing
paths. Tools/resources/prompts/completions and typed error results remain; progress/cancellation
and ping work during long operations. Protocol support follows the pinned SDK and tests, not
obsolete date claims in the old spec. No hidden headless fallback. Window screenshot/input APIs
must identify an actual attached browser/test harness or say unavailable; a rendered plan must
not be presented as a screenshot of the person's screen. Browser window focus/resize/quit cannot
be promised by a service-side command. M6 must specify the selected-browser capture adapter.

**AUTO-03.** AI Connection retains endpoint copy, named key creation/revocation through human
settings, setup instructions and connected/activity marks. Endpoint is the one HTTPS service,
not a per-window loopback listener. Never copy real keys into documentation or logs; show-once
credentials and safe environment-variable examples remain. Revoked keys lose access on subsequent
requests. Explain that returned pictures/reports become part of the chosen AI client's context.

## 10. Completion evidence

**ACCEPT-01.** [The acceptance catalogue](design-audit/acceptance.md) is a release checklist,
not a claim that tests already exist. Every requirement above maps to assertions through the real
browser/server boundary; command unit tests alone do not prove reachable, working controls.
Import/preparation and planner contracts also need geometric, failure and simulator tests.

**ACCEPT-02.** M4 finishes only after the entire required desktop inventory is reachable and
works, paired comparison evidence covers reference views/dialogs/cutting states, every carried
regression is addressed, and each exclusion is approved explicitly. Unresolved design choices
are resolved before dependent code. M5 then reaches equivalent outcomes on compact screens;
M6 applies the same contract to MCP. Hardware/browser-specific acceptance is recorded separately.
