# CutCraft specification

Status: the design reference behind CutCraft 1.0 (2026-10-24). Requirements describe intended
behaviour; [CHANGELOG.md](../CHANGELOG.md) lists what each version changed, and
[§13](#13-revision-history) what each draft of this document changed. Hardware behaviour not
recorded as measured here is still an assumption.

**Scope tags.** **v1** is the first version the end user uses for real projects: milestones M0–M6
([§11](#11-milestones)). Every requirement is v1 unless marked **[later]**. Later items keep their
design here so v1 doesn't paint them into a corner, but they don't hold up v1.

## 1. Purpose

CutCraft opens a design, makes it **cut-ready in one click**, lets the user choose what to cut by
**colour** or **layer**, shows a **Cut Preview**, and drives a **Silhouette Cameo** directly.

It replaces Illustrator + the Silhouette Connect plugin. That plugin is the most-reported pain point
in the community research: it breaks with Illustrator and macOS updates (no Sequoia build for a
while, Rosetta workarounds on Apple silicon), and it is why every project currently ends in a call
for help. CutCraft avoids that plugin dependency: save the design in a supported format,
then open (or re-open) it in CutCraft. Import compatibility still needs regression testing.

It is a cutting tool, not a design editor. Real design changes happen in VectorCraft or any other
editor; CutCraft arranges what it is given on the mat.

**The test for every feature:** could the end user do this without calling for help?

Opening, preparing, saving, previewing and cutting work offline, without an account. The original
design is never overwritten. Conversion uncertainty is shown before cutting; unsupported content
must not quietly disappear. A saved project preserves the geometry used for a repeat cut.

## 2. Users, machines and platforms

- **User:** a hobbyist making vinyl projects, not technical. Designs in Illustrator today.
- **Cutter:** an original Silhouette Cameo (2011, USB product 0x1121), confirmed 2026-10-14 by its
  case (no version number) and USB descriptors. Silhouette has ended official support for it,
  which strengthens the case for CutCraft.
- **Development machines:** a NixOS desktop (development, and the Cameo for hardware tests) and an
  Apple silicon Mac mini (macOS builds, and Intel code under Rosetta). The end user's MacBook is used as
  little as possible: once to read its macOS version, and once for final acceptance
  ([§11](#11-milestones)). Until the hardware milestones, cutting is developed against the
  simulator ([§7.4](#74-cameo-simulator)).
- **Platforms:**

| Platform                     | Priority     | Notes                                                                  |
|------------------------------|--------------|------------------------------------------------------------------------|
| macOS on the end user's Intel MacBook (i7) | **Must**  | macOS version and model year to confirm; sets the minimum macOS. Final acceptance only; developed on the Mac mini |
| macOS on Apple silicon       | Should       | Build both architectures; test installed fonts, rendering and USB on each supported target |
| Linux                        | Nice to have | Development platform; needs a udev rule                                |
| Windows                      | Nice to have | App checks in a Windows VM against the simulator. A real cutter there needs USB passthrough and a WinUSB driver (Zadig) or another route, since Windows binds the Cameo to its printer driver. Decide later |

## 3. Words the user sees

Plain language everywhere. Internal terms stay in code and this spec's technical sections.

| Avoid                         | Say instead                                   |
|-------------------------------|-----------------------------------------------|
| job                           | **project** (the design and how it is cut)   |
| pass (one loaded piece)       | **sheet** (the piece of material you load and cut) |
| number of passes              | **Cut twice / 3 times**                       |
| outline stroke                | thick lines turned into shapes                |
| expand appearance / brush     | effects turned into shapes                    |
| unite, pathfinder, weld       | overlapping pieces joined                     |
| clipping mask                 | hidden parts trimmed off                      |
| spot colour, CutContour       | cut line colour                               |
| artboard                      | page                                          |
| media                         | mat / roll                                    |
| origin                        | start corner                                  |
| raster, trace threshold       | picture, "how dark counts as black"           |
| dry run, toolpath, travel     | Preview, cut order                            |
| blade depth, force, speed, passes, mirror, overcut | editable in the right panel |

- **Units:** inches by default (2 decimals, fractions in hints: "1/8 in"); millimetres as a setting
  (View, or the toolbar's in/mm button). Lengths are labelled W and H; a field accepts "63 mm",
  "6.3 cm" or "2 1/2 in" whatever the setting.
  Inputs also accept fractions and explicit units. Display rounding never changes stored geometry;
  Material controls show enough precision for blade settings.
- **Main buttons:** Open, Cut (to the cut screen), Test cut, Stop (§4.1).

## 4. Workflow

1. **Open** a design (drag and drop or recent files). Pictures first get "Turn picture into cut
   lines".
2. **Make cut-ready** runs automatically after import checks and reports its changes. Reversible
   choices have Undo; unresolved conversion problems need a decision before cutting.
3. **Choose what to cut:** every colour (or layer) is ticked; they untick what this cut leaves
   out and chooses one material for the cut.
4. **Cut Preview:** dimensionally accurate mat, cut order, warnings; drag to place.
5. **Cut** with step-by-step loading prompts. For several colours of vinyl they untick all but
   one colour, cuts, then ticks the next and cuts again; layering marks line them up.

There is only ever one sheet: the piece of material being prepared now (agreed with the user
2026-10-19). A sheet may hold any number of colours or layers.

### 4.1 Main window

Agreed with the user 2026-10-18; it replaces separate step screens.

- **Menu bar:** File (Open, Open Recent, Save, Save As, Update from Design, Close, Quit), Edit
  (Undo, Redo, Materials), View (Side by Side, Overlay, Cut Preview; Zoom In/Out, Fit in Window;
  Show Moves Between Cuts), Cut (Cut Preview, Test Cut, Cut…, Stop), Help. Every
  item is a registered command or a view change, with its shortcut (Ctrl is ⌘ on the Mac).
- **Save:** until a project has a chosen path, Save opens Save As. Subsequent saves use that
  path; cancelling the dialog writes nothing. Save As always asks for a location.
- **Window title** names the open file (the saved project, else the design); the menu row shows
  its full path, right-aligned, and clicking it copies the path.
- **Toolbar:** Open, Save, Undo/Redo, the view switch **Side by side | Overlay | Cut preview**,
  zoom out / Fit / zoom in, **Test Cut** (a miniature square-and-triangle pattern) immediately
  left of **Cut**, the Assistant badge and Stop. Test Cut uses the same shared pattern and
  loading actions as the Cut menu; starting still requires explicit loading confirmation.
  Cut becomes **Exit** on the cut screen; Test Cut becomes **Exit** on the test-cut screen.
  Exit returns to Cut Preview, clearing reviewed jobs while retaining occupied test areas
  and completed-cut records. It is disabled during motion. Only the toolbar and Cut menu
  navigate into the test-cut screen; its panel contains execution controls, not another entry link.
- **Views (centre):**
  - *Side by side*: the artwork as drawn (rendered by the engine) on the left and the cut lines on
    the right, always showing the same region at the same zoom. Zooming in shows exactly what the
    blade will follow; rows not cut are left out of the cut lines (and the Overlay), except
    faintly while the pointer is over the row in What to cut, so the pane never shows a line the
    blade won't cut; *Show me* zooms both views to the objects an issue
    is about and outlines them (and their cut lines) in orange; pressed again it stops.
  - *Overlay*: the cut lines drawn over the artwork.
  - *Cut preview* (§5.4): the sheet on their real mat (its outer edge, sticky grid and
    rulers, from a mat profile; presets plus their own) or the roll, filling the pane without
    panning or zooming; there the toolbar's zoom out / Fit / zoom in (and Ctrl+- / 0 / =)
    make the design smaller, fit it to their piece or the mat's sticky grid inside the margins
    (moved to that corner), and make it bigger, in 10% steps. Opening the Cut preview fits cut
    lines that don't fit: too big, they shrink as Fit does; only reaching off, they move inside.
    A calm note says which, and one Undo ("Fit to the mat") reverses it. Their material is shown on the mat and is placed by dragging or by typing
    its position on the mat's rulers; dragging the design places it. A
    legend explains what is drawn; the test-cut spot is labelled, can be dragged anywhere clear
    of the artwork (gaps inside the design's box included; red and refused where it would cut
    into the design), and hidden by right-clicking it or from View or the panel without
    changing what is reserved. The artwork's box has corner handles that resize every colour
    together, with its width and height just outside each side in the chosen units; double-click
    a size to type one (linked, the top-left corner stays put). **Snap to grid** (Cut lines
    group, with its step in the chosen units, e.g. 0.25 in or 5 mm) lands dragged things on the
    grid of the mat's rulers and a resized box on whole steps.
- **Issues also ask** about very small pieces, very thin pieces and outlines that nearly repeat
  (*Cut them anyway* / *Keep both*, `accept_warning`), and about photo-like pictures. In the Cut
  preview, the sheet's warnings are cards: off the sticky grid (its edge drawn red on the mat,
  *Move onto the grid*), outside the cutting area (*Move it inside*), material not for a roll
  (*Use a 12 × 12 in mat*).
- **Cutter** picker at the very top of the right panel (and on the cutting screen): the real
  Cameo over USB, the Simulated Cameo in development builds, or Disconnect.
- **Right panel groups** fold: Issues, Info, Artwork size, What to cut, Material (Select
  Material, blade depth, force, speed, passes, Mirror, overcut, Save, Save As, Materials…), Mat, Media, Cut lines
  (placement), Cutting aids (with the Cut preview's view options) and Estimate in all three design views.
  Artwork size has its own group throughout. Mat-specific visuals and cut-order playback
  remain in Cut Preview; the shared panel explains this. Each group's header has a drag handle
  (drag to reorder) and a lock: locked groups sit above the scrolling part (at most half the
  panel, scrolling themselves beyond that) and stay in view; dropping a group among the locked
  ones locks it; the padlock carries an up-and-down arrow, since it locks the scrolling and not
  the settings. The arrangement is a saved preference (`settings.panel`); View → Reset Panel
  Layout restores the usual order.
  - Cut lines and the mat are drawn as vectors at every zoom; the artwork is re-rendered for the
    visible region at the current zoom, off the drawing thread.
- **Right panel:** **Issues** (Make cut-ready's questions and problems, §5.2: a warning triangle
  with the number still to resolve; one collapsible card per issue with a short title such as
  "Missing font", the detail inside; resolved ones turn green with a tick), **Info** (what Make
  cut-ready did on its own and notes needing no answer, one blue card each, folded), artwork
  size, what to cut and the material
  (§5.3), and the sheet's mat or roll, orientation, placement, aids, piece,
  estimate, warnings and cut order in all three design views. The toolbar has the single primary Cut control:
  **Cut** always opens the cut screen without preparing or starting a job. It is disabled
  until preparation is cut-ready and the current sheet plan is valid, including after edits
  while a new plan is pending. The Cut menu uses the same navigation and readiness rule.
  The panel retains the cutter picker and blocking issues. Material adjustment is inline.
- **Navigation (Illustrator conventions):** the wheel scrolls up and down; a side wheel or
  Shift+wheel scrolls across; Ctrl/⌘+wheel and pinch zoom about the pointer; Space+drag or
  middle-drag pans (a plain drag also pans where it does nothing else); Ctrl+0 or a double-click
  fits; Ctrl+= / Ctrl+- zoom; Ctrl+1/2/3 switch views.
- Automation reaches the same views with `view.show` (`side-by-side`, `overlay`, `mat`, `cut`) and
  the control protocol's `ui.scroll`.

## 5. Requirements

### 5.1 Opening files

| Kind   | Formats                                                        | Required validation           |
|--------|----------------------------------------------------------------|-------------------------------|
| Vector | `.ai`, `.svg`/`.svgz`, `.pdf`, VectorCraft's own (`.vectorcraft`, legacy `.drawcraft`) | First release: authored fixtures and representative real files |
| Vector | ASCII `.dxf`, model space in the XY plane | Authored unit, curve, layer, block, limit and project round-trip fixtures; physical size retained |
| Vector | `.eps` (PostScript, read by VectorCraft's PostScript reader; an Illustrator EPS's layers from its editing copy) | Authored one-inch fixture; local survey of the archive's EPS files (2026-10-23): 62 of 64 imported as vector shapes matching their PNG/SVG twins, 2 were empty files. A file read only up to an error is refused (`import.eps_partial`); a preview-only result blocks like any other |
| Vector | `.ait`, `.emf`/`.wmf`, Affinity                                | **[later]** Experimental until a format-specific fixture set passes |
| Picture | `.png`, `.jpg`/`.jpeg`, `.gif`, `.bmp`                       | Tracing ([§5.1.2](#512-pictures)); authored fixtures and representative designs from the local corpus |
| Picture | `.psd`/`.psb` (the merged image Photoshop saves after its layers, through VectorCraft's reader; layers aren't read) | Authored PSD in the tests; the archive's 23 PSD files (2026-10-23): all open, 4 too large for the 40-megapixel limit |
| Picture | `.tif`, `.webp`                                                | **[later]** Validate each decoder and frame policy |

- Import through a CutCraft adapter to VectorCraft's engine (`load_with` or `open_bytes_with`),
  explicitly setting text, page and layer options and the source path for linked assets. Preserve
  import warnings and whether the result is editable geometry or only a raster preview. A
  preview-only result must enter the picture workflow, never appear as a successful vector import.
- **DXF:** use the CAD importer in the same pinned VectorCraft revision. Import ASCII model
  space at 1:1, without fitting to a mat. Honour `$INSUNITS`; for missing, zero or unknown
  units, preview in mm and ask `dxf_units` (mm, in, cm, ft or m) before cutting. Store the answer
  in the recipe and use it for source rendering and saved projects. A unitless-DXF answer
  cannot silently override newly declared units on update.
- Unfilled DXF paths default to one centreline cut, regardless of plotting lineweight;
  filled geometry uses normal preparation. Keep visible layer names and exclude hidden,
  frozen or non-printing layers. Expand block instances through the engine before scanning
  their contents and fonts; refuse graphs exceeding 100,000 expanded objects.
- Circles, arcs, ellipse paths, polyline bulges and spline paths use the CAD geometry importer.
  Binary DXF/DWG require an ASCII DXF export. Non-planar/thick geometry and importer warnings
  about omitted or approximated content must not silently produce a cut-ready partial design.
  Paper layouts are not selected; export intended paths into model space. Platform/hardware
  and private-corpus acceptance remain separate from authored-fixture and UI checks.
- `.ai` files open through their PDF part plus Illustrator's editing data. An `.ai` saved without
  "PDF compatible" may not open fully: say so plainly.
- Prefer verified appearance fidelity over recovering editable layers. If the engine reports a
  difference between editing data and the PDF appearance, show that warning and offer the PDF
  appearance route with its layer limitations. Resolve the choice before cutting; a small reported
  visual difference can still change a critical cut. File names such as "Outlines" or "CUT" are
  never proof that text is outlined or the file is ready.
- **Fonts are checked before substitutions become permanent geometry**:
  - use embedded glyph outlines when the importer can recover them faithfully; an embedded font
    alone does not prove that every glyph or font format was supported;
  - check `text.fonts` for missing families, substituted styles and missing glyphs on content that
    will be included, including symbol instances. Also retain import-time substitution warnings:
    text imported as outlines is no longer visible to a later font query;
  - unresolved substitution **blocks cutting** until *Install font and reopen*, *Use this substitute*
    or *Cancel*. Name the replacement and highlight affected text. Offer side-by-side comparison
    only when a reliable original rendering exists; otherwise say that the original cannot be shown;
  - accepting a replacement records the decision and regenerates preparation. Empty or failed
    glyph extraction remains an error, rather than being accepted as a font substitution.
- Multi-page files: pick the page to cut. Define the page box and treatment of art outside it;
  default to the selected page's visible area. Report excluded off-page content. Exception: an
  SVG's page (its `viewBox`) is often just where the drawing started, so when a single-page
  SVG's art crosses the page edge by more than 0.5 mm, the page becomes the bounds of the art
  touching it (art entirely off the page is still left out), with *Trim the art to the page* to
  keep the file's page instead. At the reviewed
  engine revision, requesting PDF pages bypasses Illustrator editing-data recovery: test page
  selection and layer preservation together, and report lost layer information. Prefer importing
  the full layered document and selecting a page afterwards when fidelity can be preserved.
- **Watch and reload:** when the opened file changes on disk (they saved it again in their editor),
  offer *Review updated design*. Preserve settings only for unambiguous matches, show what changed,
  and require a fresh preview; see [§5.7](#57-saving-and-repeating). Defer reload while cutting.
- Ignored automatically, listed in the report: hidden objects, guides, non-printing/template layers.
  Locked objects are included.
- Missing linked files, encrypted PDFs, malformed files and unsupported appearances get specific
  recovery choices. Do not fetch remote assets automatically. Include local linked-asset changes
  in reload detection and source fingerprints.
- File extensions are case-insensitive. Ignore AppleDouble `._*`/`__MACOSX` sidecars when listing
  designs. Downloaded bundles can contain several formats of one design; do not treat these as
  separate sheets automatically. A `.studio3` file gets a clear unsupported-format message with
  guidance to use an available SVG/PDF/AI original; do not promise an export its editor cannot make.
- **Cutter software libraries.** Users may keep designs in their cutter's own software. Silhouette
  Studio's `.studio3` is proprietary and undocumented (no public specification was found, and
  Design Store purchases cannot be exported from it); it stays unsupported (decided; see [§12](#12-open-questions)). Cricut Design Space keeps projects in Cricut's cloud: there is no exportable
  project file to read, so the guidance is to open the original SVG, PNG, JPEG or PDF that was
  uploaded. Neither library is read or modified.

#### 5.1.1 Physical size

- Preserve physical dimensions through import, page selection, preparation, placement and export.
  SVG CSS pixels use 96 px/in, while points use 72 pt/in; honour `viewBox` and transforms. For DXF,
  use declared units or ask when absent. Pictures use valid declared resolution or 96 DPI,
  preserving proportions. Enlarge pictures smaller than one inch on their longest edge to one
  inch, and fit oversized pictures to the default mat with room for the weeding border.
  Explicit user sizes override these starting defaults.
- A `viewBox` alone does not establish intended physical size. Several supplied SVGs have no
  explicit width/height: show the inferred size and require the user to set or confirm it. Compare
  paired AI/SVG files in local tests; never assume an Illustrator-exported number is a point.
- Show design width and height immediately, with linked proportions by default. Resizing and
  rotation are arrangement tools. Resizing an assembled design applies to every participating
  colour sheet; it must not independently change their relative scale.
- A scale fixture for each advertised format must produce the same known dimensions in mm. Screen
  zoom is independent of physical cut size; "100%" does not promise ruler-accurate display size.

#### 5.1.2 Pictures

Many supplied designs exist only as PNG or JPEG, so v1 traces picture files. Tracing pictures
embedded in vector files, and the remaining picture formats, are **[later]**.

- **Light art on transparency.** A one-colour trace follows dark pixels. When a picture has a
  transparent background and at least 90% of its visible pixels are lighter than the threshold
  (white art for a dark shirt), the visible pixels are traced instead, in their average colour,
  and the trace note says so. Colours for the photo check are counted on opaque pixels only:
  the soft edges of a Photoshop merged image (stored on white) otherwise add shades the art
  doesn't have.
- VectorCraft Image Trace. **One colour (decal)** by default, or **Several colours**.
- Sliders: Detail, Smoothness, "how dark counts as black", Ignore white. Live preview over the
  picture. Calculate its starting size automatically, report it in Info, and allow resizing
  in Cut Preview without requiring a size question.
- Several-colour tracing gives one group per colour (not layers), so they are chosen by colour.
- Ask before tracing pictures with more than 20 exact RGB colours in visible pixels
  (alpha at least 128), or those flagged by the existing colour-flatness heuristic.
  Count shades without quantization; ignore hidden colours and alpha-only variations.
  This is a conservative warning, not proof of photographic content; Trace anyway remains available.
- Warn when the picture is small for its cut size and show thin features and small islands in the
  result. Ignore white is an explicit trace choice, separate from vector background detection.
- For animated or multi-page pictures, choose one frame/page. For layered PSD/PSB, use the composite
  image initially and say so; layer-preserving tracing is outside v1.
- The trace settings are recorded in the preparation recipe like any other decision, so the same
  picture and settings give the same shapes.
- Shirt mockups and product photos are pictures of a design, not the design. A picture that looks
  like a photo (many colours, few flat areas) gets a warning that tracing will not give clean cut
  lines, with a pointer to the original artwork. Corpus acceptance excludes mockups.
- **v1:** a picture embedded in a vector file requires explicit exclusion (*Don't cut*) before its
  sheet can cut. **[later]:** tracing it in place.

### 5.2 Make cut-ready

One step prepares supported artwork for cutting. Runs on open; the toolbar's **Make cut-ready**
button regenerates it from the imported source and the current choices. The result is a
plain-language list; reversible choices have **Undo** and every change can highlight affected art:

```
✓ Turned 12 text items into shapes
✓ Turned 3 thick lines into shapes you can cut out
✓ Joined 5 overlapping pieces of the same colour
✓ Removed 2 duplicate outlines (would have cut twice)
? Is this a background box? (8.5 × 11 in)               [Cut it] [Don't cut]
⚠ 1 picture needs conversion before it can be cut       [Show me] [Don't cut]
```

Files that need nothing show a small "Nothing to fix ✓". Per-step on/off switches live under
Advanced. A before/after view is available.

Preparation keeps an immutable imported document and a recipe of decisions. Undo changes that
recipe and recomputes affected later steps; it does not undo an arbitrary engine command after
other commands have consumed its output. Re-running the same recipe produces the same geometry
and report. Cancelling or failing a step retains the last complete result. Undoing a required
conversion may leave content uncuttable: show that state and block its inclusion until resolved.

**Steps, in order** (engine commands are run on explicitly selected objects, never on "select all";
see [§7.1](#71-make-cut-ready-implementation)):

1. **Expand instances and text → shapes**: resolve symbol instances and inherited visibility and
   transforms, then `type.createOutlines`, after checking fonts in the expanded content.
2. **Effects and brushes → shapes** (`effect.expandAppearance`, `object.expandBrush`). Blur-type
   effects (soft shadows, glows) may become pictures: require tracing or explicit exclusion.
3. **Thick lines → shapes** (`object.path.outlineStroke`) for every visible stroke **except cut
   lines**:
   - a stroke in a colour named `CutContour`/a chosen cut-line colour is a **cut line**, cut once
     along its centre;
   - a stroke on a **filled** object is **part of the shape** (agreed with the user 2026-10-19):
     outlined and joined to the fill in the fill's colour, so the piece is cut along the stroke's
     outer edge, whatever the stroke's colour or width;
   - on an unfilled object, a stroke ≤ 0.1 mm (hairline) is a cut line; all other strokes,
     including Illustrator's default 1 pt, become shapes of their own colour;
   - this is a default interpretation, with explicit *Cut along line* / *Cut around shape* /
     *Part of the shape* overrides per colour and per object. **What to cut** has a design-wide
     *Outlines on filled shapes: Part of the shape | Cut separately* setting (decision
     `outlines_on_fills`), shown when the design has outlined filled shapes; colour and object
     choices still override it. Preserve the choice when the design is resized;
   - classify using effective imported width after transforms, and handle each appearance stroke
     separately. Mixed-width strokes of the same colour must not force one interpretation;
   - define dashed lines as their visible dash segments by default; a continuous centreline requires
     an explicit choice. Caps, joins, variable widths and non-scaling strokes need fixtures.
4. **Hidden parts trimmed off** (clipping masks applied by geometry; [§7.1](#71-make-cut-ready-implementation)).
5. **Double-cut clean-up:** combine an object's same-colour fill and outlined stroke as a filled
   region, preserving its outer extent and any real holes. An explicit centreline remains a
   separate operation unless it exactly duplicates a planned cut. Remove exact duplicate contours
   only within the same sheet and operation settings (final check after choosing what to cut); never
   remove geometry from another sheet or remove intentional repeated passes. Reversed direction
   and a different starting vertex do not make a distinct closed contour. Invisible paths with no
   fill or stroke are excluded. Near-duplicates are reported, not automatically merged.
6. **Background question:** a bottommost rectangle matching the page is a candidate, shown with
   *Cut it* / *Don't cut*. There is no default: its sheet can't be cut until they answer (one
   click, shown again in the Cut Preview), so clicking through neither cuts a page-sized rectangle
   nor drops real artwork. White alone is never enough to exclude a shape: white vinyl and white
   knock-outs are legitimate artwork. Record the answer in the preparation recipe.
7. **[later] Optional "only what's visible"** (off by default): subtract
   occluding opaque shapes in original stacking order before same-colour unions lose that order.
   Exclusion from cutting does not itself change whether an object occludes artwork; provide a
   separate explicit choice. Partial opacity, blend modes and opacity masks are not Boolean clips.
8. **Join overlapping pieces of the same colour** (`object.pathfinder.unite`):
   - only filled shapes (after step 3), never cut lines;
   - grouped by **layer, colour and operation**, so nothing changes layer or combines objects of
     different colours. Changing what is cut recomputes affected unions;
   - letter holes stay holes; shapes inside holes stay separate;
   - touching script letters join into one piece (the standard "weld"); a switch keeps text pieces
     separate;
   - the joined result is split back into separate pieces (outline plus its holes) for counting,
     ordering and weeding.

Gradients, patterns, meshes, opacity masks, blend modes and partially transparent artwork need an
explicit policy. **v1:** where the shape is well-defined, they pick one material colour for it;
otherwise they exclude it. **[later]:** tested conversions and tracing. Do not silently flatten
them to the top fill or invent a colour. Unsupported content blocks the cut until resolved. Preserve source-object provenance through splits and unions so reports and later
exclusions identify the affected artwork.

### 5.3 Choose what to cut

```
 Group by: (●) Colour  ( ) Layer
 [x] ■ Red             14 pieces
 [x] ■ Gold             3 pieces
 [ ] ■ Black            1 piece, not cut
 [x] ┄ Red cut lines    2 lines
 Material: Permanent vinyl ▾   Blade / Force / Speed / Passes / Mirror / Overcut   [Save] [Save As…]
```

- One list, grouped by **Colour** or **Layer** (toggle). Colours include fills and cut-line colours;
  after Make cut-ready, strokes are shapes, so fill colour is what matters.
- Default to Colour. Suggest Layer when there are multiple meaningful named source layers; a
  synthetic importer layer is not evidence of user intent. Toggling grouping keeps the material
  and placement; a new row stays unticked only when everything in it was unticked.
- **One sheet** (agreed with the user 2026-10-19): everything ticked is cut together on the
  material being prepared. Every row starts ticked; unticking leaves it out of this cut. Cut
  lines keep their own colours as rows. There are no sheet numbers, no *Move to sheet…* and no
  "next colour" prompt: to cut colours on different vinyl they untick, cut, and tick again. A
  project saved with several sheets opens as one sheet holding every row (ticked as before) with
  the first sheet's settings; its old cut record is dropped.
- Group colours by explicit source swatch identity where available, otherwise by defined canonical
  colour values. Keep distinct spot colours distinct even when their previews look alike. Offer
  *Treat as the same colour* for near matches; do not auto-merge them by visual similarity.
- Clicking a row or piece selects and highlights it. **Don't cut** is a separate checkbox/action
  with Undo, never a side effect of starting a drag. Excluded pieces stay listed (greyed, "not cut") in What to cut, and pointing at the row shows
  their outlines faintly in the cut lines.
  For a welded piece, excluding it excludes the whole result; selecting a source object in the
  preparation view excludes that source and recomputes the weld. Display which scope is affected.
- The cut has one **material** (with its blade number) and **mirror**, chosen for the cut, not
  per colour; iron-on materials mirror automatically. One material and tool per cut; mixed-tool
  work is outside v1.
- Preserve relative positions within a design by default. A shared assembly view shows all
  colours together. Moving the design on the mat changes placement only; it does not change
  assembly coordinates, so separate cuts of different colours still line up. **[later]** Independent rearrangement, opt-in for separate decals, which cannot
  silently break lettering or layered designs.

### 5.4 Cut Preview

The required screen between choosing and cutting.

```
 Permanent vinyl · Silhouette 12 × 12 in mat
┌──────────────── 12 × 12 in mat ───────────── top goes in first ↑ ┐
│ ┌ your vinyl 6 × 8 in ┐   inch grid, dimensions shown             │
│ │  pieces drawn in their colour; cut lines dashed                 │
│ └─────────────────────┘                                          │
└──────────────────────────────────────────────────────────────────┘
 Design 5.25 × 3.10 in · Uses 5.75 × 3.60 in of vinyl · about 2 min
 ⚠ 4 pieces smaller than 1/8 in — hard to weed          [Show me]
 [▶ Play cut order ───●────]  [Test cut]  Mat ▾ 12×12 | 12×24 | Roll
                                                        [ Cut → ]
```

Must show:

- pieces in their own colour on a dimensionally accurate mat or roll with an inch grid, zoom and
  pan; cut lines distinct. This is a scaled view, not a promise of physical screen calibration;
- optional outline of their actual vinyl piece ("How big is your piece?");
- **cut order**: a play/scrub slider, inside cuts before outside ones, moves between cuts optional;
- design size, required material bounds (including cutting aids and compensation), **approximate
  time** including repeated cuts and travel. Distinguish this estimate from measured completion;
- **mirrored** badge when the cut is mirrored;
- **warnings**, each with *Show me*: tiny islands, narrow bridges or holes that are hard to weed,
  iron-on not mirrored, excluded pieces and material settings still awaiting a test cut;
- **blocking errors:** unresolved import/conversion errors, invalid geometry, nothing ticked, or
  any generated cut or machine move outside its allowed area. Known material bounds must contain
  all blade-down paths. Warnings may be acknowledged; blocking errors must be fixed;
- placement by dragging or numeric entry, aspect-locked sizing and rotation, with Undo. Default
  position reserves the weeding margin and toolpath clearance inside the actual cutting area.
  **Move to corner** translates the design as a unit; automatic rearrangement/nesting is deferred.

Represent mat bounds, usable device area, material bounds and roll roller margins separately.
Validate the final plan after mirroring, aids, overcut, blade compensation and device rounding;
never silently clamp, crop or shrink it. A 12 in mat label is not a 12 in guaranteed cutting width.
Validation runs again immediately before sending, for the connected model and current settings.

The preview and sender consume the same immutable plan. Any change to geometry, inclusion,
placement, material, mirror, preparation or device profile invalidates it. The normal preview
shows intended cut contours; an optional tool-motion overlay shows compensation and travel so
blade-holder motion is not mistaken for the finished contour. Bounds checks include both.

Cutting aids (defaults in brackets):

- **Weeding box** [on, ¼ in margin; off for the later pen/cardstock modes]: encloses included artwork
  and its layering marks, keeps clear of test cuts, and is cut after the artwork;
- **Weed lines** [off]: lines splitting waste vinyl, clipped to waste regions with a defined
  clearance from artwork. Regenerate after exclusions or placement changes; never cross kept art.
  **Invert: keep the background** [off] (agreed with the user 2026-10-19) is for stencils and
  knockouts: the shapes are the waste and the vinyl around them and in their holes is kept, so
  weed lines run through the shapes instead;
- **Layering marks** [off; suggested when some colours are left for a later cut]: four small
  solid triangles (6 mm) on the whole design's centre lines, left, right, above and below it,
  each pointing at the artwork with its tip 3 mm from it (agreed with the user 2026-10-19). They
  are weeded like the art and stay on the carrier: they mark centre lines on the garment and
  lines each layer's triangle tips up with them, then peels the triangles off after pressing.
  They sit at the same place in shared assembly coordinates in every cut that has them,
  whichever colours it holds; weed lines keep clear of them. Transform them with the cut,
  including its mirror, so they align after transfer. Mixed mirrored/unmirrored assembly
  needs a tested workflow; disable the suggestion when it cannot be represented reliably;
- **[later] Overlap** [off]: lower colours grown slightly so no gaps show.

### 5.5 Cutting

Full-screen, step-by-step:

1. "Load **red permanent vinyl**, backing paper down, on the mat. Set the blade to **1**. Choose
   *Load Mat* on the Cameo." A picture shows how. Confirm physical setup, then check readiness;
   device status alone does not establish material, blade setting or the chosen load mode.
2. "Cutting the sheet — about 60%" with large **Pause** and **Stop**. Progress is estimated
   unless the device provides verified execution progress; sending all bytes does not mean done.
3. "Done. Unload the mat." with a weeding tip, **Back to the preview**, **Cut again** and
   **Open another design**.

- **Material settings are edited directly in the right panel** in every design view and
  while the cut screen is idle: blade depth, force, speed, passes, mirrored and closed-contour
  overcut. Blade depth remains a manual dial instruction. Advanced offset controls stay available.
- Selecting a material loads its saved settings and discards unsaved material adjustments.
  Edits stay in shared session state across views and newly opened designs until another
  material is selected or the app closes. Project saves still capture the sheet's settings;
  only material **Save** / **Save As** changes the material library.
- **Default materials and mats**: **Make Default** in the Materials and Mats windows (or the
  editors' **Default** checkbox, `materials.save` / `mats.save` `default`) sets the material
  and mat every newly opened design starts with; the default is listed first and tagged
  *Default*. Saved in `preferences.json` `new_design` by name (`settings.new_design`, MCP
  `new_design_set`). Without one, or if the name has left the
  library, a design starts with the session's material (else the library's first) and the
  12 × 12 in mat. Renaming the material or mat keeps it chosen; projects keep their own.
- **Save** updates the current material. **Save As** asks for a new, unused name, saves a copy
  and selects it. Invalid names, duplicate names and failed writes preserve the draft and library.
- There is no tested/provisional tracking, result questionnaire or material-test wizard.
  Old material status fields are accepted but ignored and omitted on subsequent saves.
  The optional square/triangle **Test cut** uses the same loading and start controls as a cut,
  keeps occupied spots clear and never adjusts settings or saves a preset automatically.
- **Help → Troubleshooting** explains shallow/deep cuts, tearing, lifting, incomplete separation,
  mirror mistakes and no-cut results, with setting adjustments and physical checks. It uses
  the same `help.show` command and text as MCP (`topic: troubleshooting`).
- **Stop** responds immediately in the UI without a confirmation dialog and prevents further
  ordinary cut submissions. Show "Stopping — the cutter may finish buffered moves." Bound queued
  motion and request a validated blade-up operation when communication allows. Only report the
  final state established by the tested protocol; never assert the blade is up after a timeout or
  disconnect. Show the model's verified physical-stop instructions when needed. Stop abandons the
  cut; restarting requires a fresh load/setup decision.
- **Pause** requests the next supported safe checkpoint, normally a contour boundary with the
  blade lifted. Show *Pausing* until it is reached; don't promise the current line can always be
  interrupted. Resume is allowed only while the same connection, load and plan remain valid.
- **Interrupted cut** (disconnect, cutter off, app crash, unknown transfer outcome): "The cut was
  interrupted. Check the cutter, then load fresh material to cut this sheet again." Mark the
  sheet incomplete/uncertain, persist that and a completed cut, and offer **Cut again**, never
  automatic mid-cut replay or resume after reconnecting.
- **Unload** is manual in v1. **[later]** An app button once its command and readiness conditions
  are verified for the exact model; after that, automatic unload as an opt-in, never part of a
  test cut or an uncertain stop.
- **Save cutter file** (the raw commands) lives in an Advanced/Help menu for debugging.

One worker owns the device and all reads/writes. Discovery may reconnect the display automatically,
but never restart motion. Reject unsupported models rather than trying a similar command set;
select explicitly when more than one supported cutter is attached. Prevent concurrent app/CLI
jobs and double-click starts. Keep long imports and geometry work away from the device worker.

Device execution has explicit states: Disconnected, Unsupported, Idle, Awaiting setup, Ready,
Sending, Draining, Pausing, Paused, Stopping, Completed and Interrupted/Unknown. Define allowed
commands and timeouts in each state. Completion requires a model-validated queue-drained/idle
condition after all commands, not USB write completion alone. Preserve uncertainty if status
cannot prove completion. Record plan identity, connection identity and completed checkpoints.

While cutting, keep the active plan immutable and defer reload/settings changes. Intercept normal
app close with a choice to keep cutting or stop; handle sleep/wake and lost communication as an
interruption unless continuity is proven. Persist progress atomically when a cut ends.

### 5.6 Materials

- Shown as a name and a blade number. **v1:** **Permanent vinyl (e.g. Oracal 651)**, **Removable
  vinyl (Oracal 631)**, and Siser's iron-on and EasyPSV lines grouped by shared settings (iron-on
  mirrors automatically, "shiny side down"): EasyWeed and similar, Glitter, Holographic/Aurora,
  StripFlock Pro, EasySubli, Brick 600, Easy Puff Glitter, and EasyPSV Brushed, Etch, Glitter
  (with Holographic, Patterns, Starling), Glow and Blackboard. Siser's values are from its own
  Silhouette charts (HTV 07-09-2025, EasyPSV 5-2023); each material's notes name the products it
  covers. EasyWeed Adhesive (a clear adhesive for foils and fabric) is left out. **[later]:** **Cardstock** and **Sketch pen**, once their workflows pass material
  tests (see the sketch pen bullet below).
- Inline controls: blade depth (1–10), force (1–33), speed (1–10), passes (1–3), mirrored,
  overcut (0–2 mm). Advanced blade-offset method/value remain available.
- Presets are starting values. Users experiment with small cuts and save useful settings;
  CutCraft does not certify or track whether materials have been tested.
- Profiles include loading notes and mat/roll suitability. Choosing iron-on loads its saved
  mirror default; editing Mirrored changes the session settings and can be saved to the preset.
- Force/speed ranges come from the device profile. Blade depth is a manual instruction for a
  ratchet blade, never an implied automatic adjustment.
- Sketch pen is an operation mode: no blade compensation, overcut or weeding aids by default.
  Specify whether to draw centre lines or shape boundaries; do not run a blade preparation recipe
  unchanged. In v1 the pen is a developer setting used for driver validation, not a material they
  sees.
- Users can add, edit, duplicate and delete materials.

### 5.7 Saving and repeating

- **Save project** (`.cutcraft`) stores a versioned, self-contained snapshot of prepared geometry,
  source provenance, preparation decisions, what is cut, material values, placement, mirror, exclusions,
  assembly coordinates and aids. Include the imported source representation and local assets
  needed to regenerate preparation; preserve resolved text geometry so repeat cutting does not
  require the original fonts. Store source location/fingerprints and importer/planner versions.
- Reopen the saved snapshot even if the source file moved, changed or vanished. *Update from design*
  is separate from *Cut again*. Repeating still requires current device validation, preview and
  loading checks. Stored raw commands are never automatically trusted on another device/profile.
- On update, **v1** keeps the sheet's settings, and unticked rows only where rows (colours or layers) match
  unambiguously, asks about the rest, and clears per-object exclusions with a notice listing them.
  **[later]:** match individual objects by stable identity so per-object choices survive updates.
  Duplicate layer names, renamed colours and changed geometry can make a match ambiguous: show
  additions/removals and ask how to assign them. Never silently transfer a per-object exclusion to
  a different object. Keep the prior snapshot until the update is accepted.
- Material profiles are copied into the project. Editing a global preset does not silently change
  existing projects; offer an explicit update. Record acknowledged substitutions and warnings;
  invalidate them when the affected content or settings change.
- Save atomically, so a crash leaves the last complete project. Files carry a schema version; a
  newer unsupported schema opens with a clear error and leaves the file untouched. **[later]:**
  migrating older schemas forward (v1 has only one). Unsaved changes get a Save/Discard/Cancel
  choice on closing the project.
- Recent designs and projects under File ▸ Open Recent and the toolbar's Open Recent (moved files
  are greyed out with their path); with nothing open the workspace shows only a quiet hint.

### 5.8 Errors and help

Every error says what happened and what to do. Offer **Try again** only when repeating the action
is safe; sending an uncertain cut again is not a generic retry. **Copy problem report** includes
app/engine versions, OS, device/firmware, plan identifier and a bounded status/error log. Omit full
paths, artwork, source text and raw cutting coordinates by default. A separate explicit export can
include raw commands for diagnosis. Reports stay local until the user shares them.

| Situation                         | Message (essence)                                                          |
|-----------------------------------|-----------------------------------------------------------------------------|
| Cutter not found                  | "Can't see the Cameo. Check it's on and the USB cable is in." Keeps looking; clears itself |
| Interface busy                    | "Another app or a system driver may be using the cutter. Close Silhouette Studio or Silhouette Connect, then Try again." Do not claim to know the owner without evidence |
| USB permission denied             | Give platform-specific setup instructions; distinguish from an occupied interface |
| Unsupported model                 | Name the detected model and supported model; sending is disabled |
| Connection drops                  | See Interrupted cut above                                                   |
| Missing font                      | Blocking choice, see [§5.1](#51-opening-files)                              |
| Unreadable `.ai` without PDF part  | "We couldn't read this file's editing data. Re-save with 'PDF compatible', or export SVG/PDF." Only show this diagnosis when the importer established it |

### 5.9 Responsiveness and accessibility

- View → Interface font size adjusts menus, panels and dialogs from 10 to 24 points
  (default 14), applies immediately and persists across launches. The shared
  `settings.font_size` command and MCP `font_size_set` tool take `points`.

- Opening, preparing, tracing and planning show progress and can be cancelled without losing the
  previous result. Bound decoded input sizes, geometry counts and preparation work; reject files
  beyond the supported budget with an actionable message. Define those limits from the M1 corpus.
- Initial acceptance targets, measured on the Mac mini (natively and under Rosetta) during
  development and confirmed on the end user's MacBook at acceptance: acknowledge input/Stop within 100 ms,
  keep preview interaction at 30 fps on the agreed normal-size corpus, and show progress for work exceeding
  500 ms. These are targets to measure, separate from physical cutter stopping time.
- Keyboard access, visible focus, readable text and non-colour status indicators are required.
  Provide buttons/numeric entry for drag actions. Test display scaling in the egui platform spike
  (M0); **[later]:** screen-reader support for the main workflow, with gaps recorded in the spike
  before anything is promised.

### 5.10 AI control through MCP

- The MCP service starts by default with the native UI. Saved settings control on/off and port;
  changes apply in the running app. Bind only to loopback and show running/error/disabled status,
  the endpoint and port conflicts in the window. Disabling it does not disable human use.
- AI tools use the same application actions, current design and validation as the human controls.
  Worker results cannot replace a newer project revision. Existing features must be available
  through MCP as well as the UI; unimplemented preparation/cutting must not be advertised as tools.
- Use standard Streamable HTTP MCP, validate Host/Origin and support stdio-only clients with a
  bridge to the existing window. Do not create a second hidden project state or expose an
  arbitrary shell/engine-command escape hatch. MCP never bypasses hardware/motion acceptance gates.
- Provide in-app help and a checked-in guide for Codex, Claude Code, Claude Desktop and VS Code,
  covering connection checks, saved settings, port changes, unavailable renderers and troubleshooting.
  Return actual rendered screenshots and structured errors/diagnostics. Private material stays
  local unless supplied to the explicitly connected AI client; it must never enter source control.

### 5.11 Cutting on cameo-pi

[cameo-pi](https://github.com/mkrzywonski/cameo-pi) is a network cut server: a Raspberry Pi
plugged into the Cameo, so the cutter can sit in another room. Each of its cutters is just another
cutter in CutCraft (cameo-pi spec §11). CutCraft prepares and plans as always; the Pi validates
the plan it receives and cuts exactly that plan. The client is `crates/station`.

- **Pairing:** *Add cameo-pi…* in the cutter list lists the Pis found on the network (Bonjour,
  `_cameo-pi._tcp`), each with *Pair*, and also takes a Pi's name or address
  (`cameo-pi.local`). CutCraft shows the code the Pi shows. The user allows it on the Pi's web
  interface, and after that there's nothing to type.
  - CutCraft asks as "CutCraft on <this computer's name>".
  - It keeps the Pi's key and certificate fingerprint in `stations.json` in the settings
    folder. The key never goes into projects, logs, the journal or MCP output.
  - The certificate is pinned. If it changes, CutCraft says "cameo-pi's identity changed …
    Pair again" and sends nothing, never re-trusting silently.
  - *Forget* removes a Pi.
- **The cutter list** shows each paired Pi's cutters, under the names their user gave them:
  "Workshop Cameo (on cameopi)".
- **Cut and Test cut** use the same screens, loading steps and controls as a USB Cameo.
  1. **Start sends the job.** The project, as it is now, and the reviewed plan go into the Pi's
     library, in its **CutCraft** folder, named after the design. Sending the same open design
     again makes a new revision of that job, not a second job. Nothing is sent before Start.
  2. **It asks for a turn on the cutter.** When someone else has the cutter, CutCraft shows
     "In line: 2nd for Workshop Cameo", and Start waits. When the turn comes, CutCraft says so,
     and the user presses Start again.
  3. **A cut needs a fresh mat** (cameo-pi §8.2): one the Pi saw being loaded since the last
     cut. Otherwise CutCraft shows "Load a fresh mat on Workshop Cameo, then press Start." A test
     cut and then the cut work on one load, as on USB.
     - **A mat loaded while the Pi wasn't watching** (after it restarted) offers **It has fresh
       material**, confirmed once (`cut.confirm_fresh`). It's never offered for a mat that was
       cut on.
  4. **Starting** names everything the user saw: the turn, the revision, the mat and the Pi's
     boot. The Pi refuses the start if any of it changed. CutCraft never starts motion on its
     own: each start follows a Start press made after the mat was loaded.
  5. **Progress, Pause and Stop** come from the Pi's state, read about four times a second.
     The outcome is read from the job's history on the Pi: finished, stopped, interrupted, or
     not run.
  6. **The turn ends** after the main cut. Leaving the cut screen, disconnecting or closing a
     test-only turn hands the cutter to the next in line.
- **Test cuts:** the Pi places them, on the same rule as CutCraft (the suggested spot if it's
  free, otherwise the first clear spot). They avoid test cuts already on the turn's mat.
- **Send to cameo-pi** (on the Cut screen and in the File menu) stores the prepared job in a
  library folder the user chooses, without cutting.
- **Material and mat lists:** CutCraft sends its lists to every paired Pi automatically. That
  happens on pairing, when it looks at the Pis, on connecting, before sending, and after a
  material or mat is saved or deleted, and only when they changed since the last send.
  - The Pi uses them for Adjust on its web page (cameo-pi spec §12.1). There, a job's size,
    place, material settings, mat, piece and weeding box can change for a re-cut, as a new
    revision.
  - The Pi never edits the lists.
- **MCP parity:** every step above is a command: `station.discover`, `station.pair`, `station.list`,
  `station.forget`, `station.library`, `device.connect` with `device: remote`, the usual `cut.*`
  commands, and `cut.send`.

## 6. Architecture

Rust (edition 2024) Cargo workspace. Library crates have no UI dependencies.

| Crate              | Responsibility                                                              |
|--------------------|-----------------------------------------------------------------------------|
| `cutcraft-import`  | Open files, font check, Make cut-ready, change report (on `vectorcraft-engine`) |
| `cutcraft-job`     | Sheets, grouping by colour/layer, materials, placement, `.cutcraft` files   |
| `cutcraft-path`    | Flattening, blade compensation, overcut, cut order, weeding box/lines, layering marks, time estimate |
| `cutcraft-device`  | Cameo driver: model table, GP-GL encoder, status, chunked sending; USB transport (`nusb`) behind a trait |
| `cutcraft-sim`     | Cameo simulator behind the same transport trait: protocol, machine state, timing, faults, virtual mat ([§7.4](#74-cameo-simulator)) |
| `cutcraft-control` | Shared UI/MCP application actions, live state, worker revisions and saved settings; from M2 the command registry: ids, schemas, enablement, authority classes, journal ([§6.2](#62-automation-and-mcp)) |
| `cutcraft-mcp`     | MCP service in the running app (Streamable HTTP), stdio bridge, and later a headless server ([§6.2.3](#623-mcp-server)) |
| `cutcraft` (app)   | egui/eframe UI; hosts the default-on loopback MCP service ([§5.10](#510-ai-control-through-mcp)); optional control protocol ([§6.2.6](#626-control-protocol)) |
| `cutcraft-cli`     | `prepare` (report), `plan` (preview SVG / cutter file), `probe` (USB), `cut`, `inspect`, `simulate`, `mcp-bridge`, `commands`, `run`, `mcp` |

Define the shared contracts before implementing the UI:

- **Imported design:** immutable source document, page/layer identities, resolved units, import
  diagnostics and fingerprints. Keep VectorCraft-specific command handling inside the adapter.
- **Prepared design:** closed regions with containment/holes, open cut lines, source provenance,
  semantic colour keys, preparation recipe and decisions. CutCraft identifiers are not transient
  engine node numbers; a union may have several source IDs and a split several derived IDs.
- **Project/sheet:** inclusion and operation choices, material snapshots, shared assembly
  coordinates, placement transforms and revision. One sheet has one tool/material/load setup.
- **Cut plan:** immutable ordered operations in mm, contour boundaries/checkpoints, intended
  geometry, compensated motion, bounds, estimates, diagnostics and the input/profile revision.
- **Encoded plan:** quantized commands plus validated bounds, plan identity and checkpoint mapping.
  Only a validated plan matching the connected profile can enter the sender; UI and CLI share
  this gate. Diagnostics have stable codes, severity and affected source/piece IDs.

Keep dependencies acyclic: `cutcraft-job` owns shared domain types without depending on the engine,
planner or driver; import, path and device use those types. The app/CLI compose the stages. Device
profiles expose planning limits as domain data. Workers report progress/cancellation/results to
the UI; a stale worker result cannot replace a newer project revision. CLI `prepare`/`plan` are
offline; `probe` performs only the verified non-motion query sequence, and only `cut` starts a cut.
`cutcraft-control` depends on the domain crates but not on egui; the app, CLI and MCP server all
dispatch through it ([§6.2.1](#621-everything-is-a-command)).

### 6.1 VectorCraft dependency

- Pin the full VectorCraft git revision and record the tested features and toolchain. The earlier
  probe at `540505fc` built without UI or native (`-sys`) dependencies in that dependency graph;
  its ≈3½ min cold release build and ≈66 MB unstripped binary are observations, not target-platform
  guarantees. Repeat build/import checks on the Mac mini (both architectures) before adopting the
  revision.
- Commit CutCraft's own resolved `Cargo.lock` and use `--locked` in CI/release builds. Compare with
  the tested upstream versions when establishing it; copying another workspace's lockfile alone
  is not a reproducibility policy. Pin the Rust toolchain and Nix development inputs too.
- The macOS and Windows installed-font listers live in VectorCraft's app crate; CutCraft needs its
  own copy, or text uses substitute fonts on those platforms.
- Engine commands take JSON parameters with no compile-time checks: **golden tests** cover every
  command CutCraft uses, so a VectorCraft bump that changes them fails loudly.
- Wrap those commands in typed adapter functions and test resulting geometry, warnings and
  provenance as well as response JSON. Dependency updates require the local import corpus and
  platform smoke tests, with the tested revision recorded.

### 6.2 Automation and MCP

CutCraft follows its sibling Craft apps (VectorCraft, GridCraft, PhotoCraft): every user action is
a command, and a local [Model Context Protocol](https://modelcontextprotocol.io) server exposes
those commands. An AI agent can then open designs, run Make cut-ready, read the report, answer
decisions, choose what to cut, build plans and look at the Cut Preview, as they would in the window.

Uses: development and corpus testing (M1–M3), remote help when a file "won't cut right", and
checking the UI against the test in [§1](#1-purpose). They never need automation, and no part of
the normal workflow depends on it.

**Parity: automation can do anything the end user can.** Every command the window offers, including
Cut, Test cut, Pause/Resume, Unload and confirming that material is loaded, is available
to MCP, the control protocol and `cutcraft-cli run`, with the same validation and the same
preconditions as their own clicks (a valid plan, the cutter reporting media loaded, the material
prompts). Nothing is refused because an AI asked. Automation's commands are recorded with their
origin, so the user can see what it did. The cutter still needs material loaded by hand; an
agent should say what it is about to cut. (Draft 3.7 replaced the earlier rule that automation
never moves the cutter, at the developer's request.)

#### 6.2.1 Everything is a command

- Every user-visible action is a command in a UI-free registry in `cutcraft-control`. Each
  command has:
  - an id (`design.open`, `prepare.decide`, `plan.build`…), a label, a menu
    path and a shortcut;
  - parameter and result schemas;
  - an enabled state with a disabled reason;
  - an **authority class** ([§6.2.2](#622-authority-classes)).

  Buttons, menus and dialogs call commands; panels read state and never change it directly. A
  drag previews while it moves and commits one command on release.
- Parameter and result schemas are JSON Schema generated from the typed Rust structs, so MCP
  advertises real `inputSchema`/`outputSchema`. The siblings describe parameters in prose; CutCraft
  doesn't. Unknown keys are rejected at every level, including nested parameters.
- Results carry CutCraft's stable identifiers, never transient engine node numbers ([§6](#6-architecture)).
  They also carry the project `revision` after the command, and diagnostics with stable codes.
- Edit commands accept an optional `expectRevision`. If the project changed in the meantime (they
  edited in the window, or a reload finished), the command fails with `revision-mismatch` and
  changes nothing.
- Undo goes through the preparation recipe and project history ([§5.2](#52-make-cut-ready)). Each
  automation command is one undo step labelled with its origin; an atomic batch is one step.
- A session **journal** records each command: id, parameters, origin, revision and outcome. It
  omits artwork, source text and paths outside the automation roots. It is replayable against the
  same source and answers "what did the assistant change?".
- A test fails when a menu item, button or dialog action has no command, or a command lacks schemas
  or an authority class.

#### 6.2.2 Authority classes

Classes describe what a command does. They label commands in `command_list`, the journal and the
assistant marks; they do not limit automation.

| Class | Examples | From automation |
|-------|----------|-----------------|
| `read` | Inspect, report, rows, plan summary, render preview, materials, cached device status | Allowed |
| `edit` | Answer a decision, assign/exclude, material, mirror, placement, resize, cutting aids, Make cut-ready, undo/redo | Allowed; marked with its origin |
| `file-read` | Open a design or project, import materials | Allowed; limited only if the session has a read root ([§6.2.7](#627-file-access-and-privacy)) |
| `file-write` | Save project, export preview SVG, cutter file or report, screenshot to a path | Allowed; limited only if the session has a write root; never a source design, for anyone |
| `device-query` | `device.probe`: the verified non-motion query sequence ([§7.3](#73-cameo-driver-clean-room)) | Allowed once the sequence is verified, as for the end user |
| `device-stop` | Stop | Allowed |
| `motion` | Cut, optional Test cut, Pause/Resume, Unload, Cut again, confirming material is loaded, acknowledging warnings that gate a cut | Allowed; marked with its origin |

- **One dispatcher for every origin.** Origins are the person's own input, control connection,
  MCP and `cutcraft-cli run`. The origin is recorded, never used to refuse: a command that works
  from their click works from a tool, batch step, `$N` reference or `ui.menu.invoke`, and fails for
  the same reasons (disabled, invalid, stale revision).
- **One exception: AI access itself.** MCP may read the AI connection settings but cannot change
  them (`settings.mcp` with changes), make access keys (`settings.mcp_key_create`) or delete them
  (`settings.mcp_key_delete`): an AI tool must never grant itself access. These are done by the end user in
  Edit ▸ AI Connection (MCP)… (or the token-protected test control port). Requests then need one
  of their named access keys when they require them, and always for a network address; only key
  fingerprints are stored. A port already in use turns AI tools off in that window
  (`port-in-use`), with the reason in the settings.
- `device-query` runs only through the device worker, which owns the device ([§5.5](#55-cutting)). It
  needs the Idle or Disconnected state and a probe sequence already verified on the hardware.
- **Fake device sessions** use the Cameo simulator ([§7.4](#74-cameo-simulator)): the
  `device.connect {scenario?, clock?}` command connects the **Simulated Cameo**, in the window or
  headless, with a scripted fault scenario and a real-time or virtual clock. Motion commands run
  against it exactly as against hardware, so agents can exercise the cutting screens, Pause, Stop
  and interruption handling. A fake session cannot open USB. The simulator is compiled only with
  the `dev-automation` Cargo feature: on by default in development builds, and turned off for the end user's
  installable build.
- `cutcraft-cli cut` asks for confirmation naming the sheet, material and device when run in an
  interactive terminal; `--yes` skips it for scripts and agents.

#### 6.2.3 MCP server

The running app serves Streamable HTTP MCP on loopback by default ([§5.10](#510-ai-control-through-mcp)).
`cutcraft-cli mcp-bridge` relays stdio-only clients to that live service. `cutcraft-cli mcp` will
serve newline-delimited JSON-RPC 2.0 on stdio for headless sessions. Protocol revision `2025-06-18`
is preferred; `2025-03-26` and `2024-11-05` clients are answered in their own version, as in the
sibling apps. Logs go to stderr. The server is a library crate (`cutcraft-mcp`) over the shared
controller, so tests and other hosts can embed it.

| Mode | Endpoint or command | What it drives |
|------|---------------------|----------------|
| Live app (default with the window) | `http://127.0.0.1:<port>/mcp`, or `cutcraft-cli mcp-bridge [--port N]` for stdio clients | The running app's current design, through the same controller as the window |
| Headless | `cutcraft-cli mcp [--in design.svg \| project.cutcraft]` | An in-process session with no window and no USB |
| Fake device | `cutcraft-cli mcp --fake-device <scenario>` | Headless with the Cameo simulator (development builds only) |

- **Live-service access:** it binds only to loopback and rejects requests whose Host or Origin
  is not local, which stops web pages and DNS rebinding from reaching it. It has no token: any
  local process for the same user can connect, as with the sibling apps. That is acceptable
  because its tools are typed actions under the authority classes ([§6.2.2](#622-authority-classes)),
  with no shell, raw engine command or motion route. Port conflicts are shown; the service never
  moves to another port by itself.
- **No silent fallback.** The bridge fails when the live service is unreachable, and the headless
  server never presents itself as the window. VectorCraft instead falls back from remote to
  headless. CutCraft doesn't, so an agent never believes it is looking at their window when it isn't.
- `initialize` advertises `tools`, `resources` (with templates), `prompts` and `completions`.
  MCP logging is not offered: the protocol is removing it (SEP-2577) and the pinned SDK marks it
  deprecated; the session journal answers what ran.
- **Requests are read on their own thread from the start.** VectorCraft's MCP notes explain how a
  loop that reads one line and answers it before reading the next rules out progress and
  cancellation. Open, prepare, plan and render report `notifications/progress` when the request
  carries `_meta.progressToken`, at most ten per second. They stop on `notifications/cancelled`
  and keep the previous complete result ([§5.9](#59-responsiveness-and-accessibility)). `ping` is
  answered while work runs.
- **Errors:**
  - unknown tools and unknown or ill-typed arguments return `-32602` before anything runs;
  - malformed JSON returns `-32700` with `id: null`, and the next line is still read;
  - unknown methods return `-32601`;
  - command failures return `isError: true` with a readable message and a diagnostic code;
  - a caught panic becomes a tool error, and the session keeps serving (no-panic policy as in the
    siblings).
- **Remote transport failures** say whether the request may have been applied. Edits are never
  resent automatically; reads may be retried once on a fresh connection, as PhotoCraft's bridge does.
- **Annotations:** every tool has a title and all four hints (read-only, destructive, idempotent,
  open-world). Tools that can overwrite a file are marked destructive. The hints describe tools;
  they don't grant permission.
- List results carry the `2026-07-28` result and cache fields for clients that request that
  revision (private, ten minutes). Resource reads are live.

#### 6.2.4 Tools

**Common tools.** These use the names the Craft apps share (FilmCraft #28 conventions):

| Tool | Arguments | Notes |
|------|-----------|-------|
| `command_list` | `filter?`, `enabled_only?` | `[{id, label, menu, shortcut, params, result, enabled, disabledReason, authority, automation}]`; `automation` is `allowed`, `roots` or `refused` |
| `command_run` | `id`, `params?`, `expectRevision?` | Runs any command the origin may run |
| `command_batch` | `steps: [{id, params?}]`, `stop_on_error?` (default true), `atomic?`, `label?` | `$N.path` / `$last.path` take values from earlier results, as in VectorCraft. With `atomic: true`, a failure rolls the whole batch back and it is one undo step; without it, earlier steps stay applied |
| `doc_inspect` | `depth?` | Project summary: source, page, size, the sheet, rows, pending decisions, plan validity, revision, history |
| `render_preview` | `view`, `max_side?`, `order?`, `highlight?` | See below |
| `ui_inspect`, `ui_screenshot` | `{}`, `{path?}` | Live app only; tool errors in headless mode |
| `undo`, `redo` | `steps?` | History, including preparation recipe changes |

**CutCraft tools**, each a thin wrapper over one or a few commands:

| Tool | Arguments | Does |
|------|-----------|------|
| `design_open` | `path`, `page?`, `route?` (`editing-data` / `pdf-appearance`), `width?` / `height?` | Imports through the adapter ([§5.1](#51-opening-files)) and runs Make cut-ready. Returns import diagnostics, `previewOnly`, inferred physical size and whether it needs confirming, the change report and pending decisions |
| `project_new`, `project_open`, `project_save` | `{}`, `path`, `path?` | `.cutcraft` files ([§5.7](#57-saving-and-repeating)); saves are atomic |
| `prepare_report` | — | The change list as the end user sees it: per change an id, plain text, technical detail, affected source ids and `undoable`; pending decisions with their allowed choices |
| `prepare_decide` | `decision`, `choice`, `scope?` | Background *Cut it* / *Don't cut*, font substitute, appearance route, picture exclusion, *Cut along line* / *Cut around shape* per colour or object, *Treat as the same colour*. Recorded in the recipe with its origin; regenerates affected steps |
| `prepare_undo` | `change` | Reverses one reversible change |
| `prepare_run` | — | The **Make cut-ready** button: regenerate from the imported source and current recipe |
| `rows_list` | `group_by?` (`colour` / `layer`) | Rows with colour key, swatch name, piece and line counts, inclusion |
| `cut_include` | `ids`, `include` | *Don't cut* and its reverse; the result names the scope affected (piece, source object or weld) |
| `sheet_set` | `material?`, `mirror?`, `mat?` (`12x12` / `12x24` / `roll`), `vinyl_size?`, `weeding_box?`, `weed_lines?`, `layering_marks?` | The sheet's settings ([§5.4](#54-cut-preview)) |
| `design_size` | `width?` or `height?` | Linked proportions; applies to every colour together ([§5.1.1](#511-physical-size)) |
| `sheet_place` | `x?`, `y?`, `rotate?`, `corner?` | Placement on the mat; `corner: true` is *Move to corner* |
| `material_list`, `sheet_adjust`, `material_save_current` | `{}`, cut settings, optional new `name` | Session adjustments; explicit Save or Save As to the library. No tested-status tracking ([§5.5](#55-cutting)) |
| `plan_sheet` | — | Plan id, design/material/device bounds, estimated time, cut and travel counts, cut-order summary, warnings, blocking errors and validity |
| `export` | `kind` (`preview-svg` / `cutter-file` / `report`), `path?`, `overwrite?` | Without `path` the bytes return as `dataBase64`. A cutter file is for inspection only; nothing in CutCraft sends a file it didn't plan |
| `device_status` | — | Cached worker state ([§5.5](#55-cutting)): state, model, firmware, connection id, last status reply and error. Sends nothing over USB |
| `device_probe` | — | `device-query` class; only with `--allow-device-query` |
| `device_stop` | — | Stop, as the window's button. Reports the state the protocol establishes, never "blade up" after a timeout |

- **Units:** length parameters take millimetres as numbers, or strings with units parsed as the UI
  parses them (`"1/8 in"`, `"3.2 mm"`). Results give millimetres plus `display` strings in their
  chosen units. Device units never appear.
- **Coordinates:** mat coordinates in mm, origin at the start corner, x across the mat and y along
  the feed. Assembly coordinates are separate and named so; transforms are explicit in results.
- `render_preview`:
  - `view` is `sheet`, `assembly`, `source`, `before-after` or `tool-motion`;
  - `max_side` is 64–2048 px, default 1024;
  - `order` (0–1) scrubs the cut order, as the play slider does;
  - `highlight` takes piece or source ids, as *Show me* does.

  It returns PNG image content plus a JSON legend (scale in px/mm, colours, warnings drawn). It
  renders from the same immutable plan as the window ([§5.4](#54-cut-preview)).

#### 6.2.5 Resources, prompts and completions

| URI | Reads |
|-----|-------|
| `cutcraft://project` | `doc_inspect` summary |
| `cutcraft://commands` | The live command catalogue with schemas, enablement and authority |
| `cutcraft://report` | The Make cut-ready report and pending decisions |
| `cutcraft://materials` | Saved material presets |
| `cutcraft://device` | `device_status` |
| `cutcraft://sheet` | The sheet: material, mat, placement, aids, plan summary |
| `cutcraft://piece/{id}` | One prepared piece: source ids, colour key, bounds, holes, inclusion |
| `cutcraft://source/{id}` | One imported source object: kind, paint, layer, what preparation did to it |
| `cutcraft://command/{id}` | One command with its schemas |
| `cutcraft://diagnostic/{code}` | A diagnostic's meaning and what to do, in the same wording as [§5.8](#58-errors-and-help) |

An unknown URI returns `-32002`; a template value that names nothing returns `-32602`.

| Prompt | Arguments | Workflow |
|--------|-----------|----------|
| `prepare-design` | `path` | Open, review the report and list the decisions a person must make. Don't answer them unless asked |
| `diagnose-file` | `path`, `symptom` | The "it won't cut right" helper: import diagnostics, fonts, sizes, rows, plan warnings, with previews |
| `check-corpus` | `folder`, `limit?` | Open each design headless and record diagnostics, size and rows; write a summary to the write root. Follow [§9.2](#92-local-project-corpus) |
| `layered-project` **[later]** | `colours?` | Cutting one colour at a time, layering marks and mirror review for a multi-colour design |
| `review-ui` **[later]** | `screen?` | Remote: a screenshot walkthrough judged against the test in [§1](#1-purpose) |

`completion/complete` covers command ids, material names, piece/source ids,
decision ids and their choices, diagnostic codes and preview views, ranked by prefix then
substring.

#### 6.2.6 Control protocol

`cutcraft --control <port> --control-token-file <path>` lets test scripts and UI tools drive the
running app below the MCP tool level, with injected input and dialogs. The default-on MCP service
([§6.2.3](#623-mcp-server)) neither uses nor needs it. Because injected input reaches more than
typed tools do, this port stays off by default and requires a token.

- **Access:**
  - It listens on loopback only. There is no preference that enables it, so their normal launch
    never listens.
  - The first request must be `auth` with a 256-bit token, as in PhotoCraft. VectorCraft's
    unauthenticated port is too weak for an app attached to a cutter.
  - A missing token file is created with mode 0600. The token never appears on the command line
    or in logs.
- **Framing:** one JSON request per line, one reply per request:
  `{"id", "ok": true, "result"}` or `{"id", "ok": false, "error", "code"}`.
- **Limits:** a line that isn't a request object closes the connection, which stops a web page's
  cross-origin `fetch` from smuggling a command. Lines over 4 MiB are refused. At most four
  connections are served, and idle connections close after 30 s.
- **Methods:**
  - `auth`;
  - `engine.execute {command, params}`, `engine.commands {search?}`, `engine.journal`;
  - `document.inspect`;
  - `ui.inspect` (screen, open dialog, focus, badge state, running work);
  - `ui.screenshot {path?}`, `ui.render`;
  - `ui.pointer`, `ui.key`, `ui.text`, `ui.scroll` (the wheel, with modifiers: Ctrl zooms, Shift scrolls sideways);
  - `ui.menu.list` / `ui.menu.invoke`;
  - `ui.dialog.set` / `.confirm` / `.cancel`;
  - `ui.resize`, `ui.focus`;
  - `app.open`, `app.save`, `app.quit`.

  All of them pass through the authority checks in [§6.2.2](#622-authority-classes).
- **What the end user sees:**
  - While a client is connected, the window shows a small **Assistant connected** badge. Its
    **Disconnect** button closes every connection.
  - Changes made by automation carry an assistant mark in the report and history, so they and
    the developer can see what it decided.
  - Their own input is never blocked. An automation edit that conflicts with theirs fails with
    `revision-mismatch`.
- **While cutting:** from Sending until the sheet is Completed or Interrupted, edit commands are
  refused with "cutting in progress", as their own edits are deferred ([§5.5](#55-cutting)). Reads
  keep working, so an agent can watch progress. Quitting over control during a cut is refused.
- **File dialogs:** a command that would need a native file dialog fails with "pass a path"
  instead of opening one for automation.

#### 6.2.7 File access and privacy

- `--automation-read-root <dir>` and `--automation-write-root <dir>` behave as in the sibling
  apps, so one client configuration works across the suite:
  - paths are made absolute, links are followed, and roots are compared by whole folder names;
  - each root must be an existing folder;
  - roots are optional limits: a root left out leaves that access as wide as theirs;
  - the live service uses the roots the app was started with; a bridge cannot widen them.
- **When a read root is set, every read of a file is checked,** not only the file named in the
  request. That includes linked assets, a source located on reopening a project, and a design
  reloaded by watch and reload. A linked file outside the read root shows as missing.
- **Writes never target an open source design or its linked assets,** even inside the write root:
  the original is never overwritten ([§1](#1-purpose)). Saves and exports refuse an existing file
  unless `overwrite: true` is passed.
- **Whatever a tool returns goes to the agent's model provider.** That includes previews of their
  artwork, text content and file names. The setup guide says so plainly. Running an agent over their
  project archive is a per-folder decision by the user. The rules in [§9.2](#92-local-project-corpus)
  still apply, and nothing an agent produces from the archive is committed.
- *Copy problem report* ([§5.8](#58-errors-and-help)) says whether automation was connected and
  includes the journal's command ids and outcomes, without parameters by default.

#### 6.2.8 Batch CLI

```sh
cutcraft-cli commands [--json] [--search TEXT]
cutcraft-cli run --in design.svg --cmd 'prepare.decide={"decision":"background","choice":"dontCut"}' \
  --cmd 'sheet.set={"material":"Permanent vinyl"}' --save out.cutcraft --export preview.svg
cutcraft-cli run --in project.cutcraft --script steps.jsonl
```

- `run` prints one JSON line per step and exits non-zero on the first failure.
- It uses the same registry and `$N` references as MCP (origin `cli-run`) and can run every
  command.
- The existing `prepare` and `plan` subcommands become thin wrappers over these commands, so the
  CLI, MCP and the window cannot drift apart.
- `cut` stays a separate subcommand ([§6.2.2](#622-authority-classes)).

#### 6.2.9 Scope

- **v1, in milestone order:**
  - M1 (implemented): the live-app Streamable HTTP service, stdio bridge, settings and help of
    [§5.10](#510-ai-control-through-mcp), with interim `cutcraft_*` tools over `cutcraft-control`.
    They move onto the registry, with the tool names of [§6.2.4](#624-tools), in M2;
  - M2: the command registry with schemas and authority classes, before the UI depends on it;
    retrofitting is the expensive part;
  - M2: headless MCP with the common and CutCraft tools, resources, `render_preview`, batch `run`,
    the `prepare-design`, `diagnose-file` and `check-corpus` prompts, and completions;
  - M3: the control protocol, UI tools and the Assistant connected badge;
  - M4: `device_status`, `device_stop`, fake device sessions, and `device_probe` once its query
    sequence is verified.
- **[later]:**
  - `resources/subscribe`, notified on project revision changes;
  - the `layered-project` and `review-ui` prompts;
  - elicitation, so a client can put a pending decision to the person;
  - sharing a schema-generation crate with the sibling apps if they adopt one.
- **Out of scope:** MCP or control reachable beyond loopback, legacy HTTP+SSE transport, and
  `sampling/createMessage`. (Automation moving the cutter is in scope since draft 3.7.)

## 7. Technical notes

### 7.1 Make cut-ready implementation

- Commands act on the selection: `select.set {ids}` with an explicit list per step and per
  (layer, colour, sheet) group; skip a step when its list is empty (commands error when nothing
  applies).
- **Outline Stroke** outlines every selected stroke, hairlines included: separate operation roles
  first. Selection by object alone is insufficient when an object has several appearance strokes;
  isolate the relevant appearance geometry before invoking the command.
- **Unite** across layers moves results into one layer and deletes open/stroke-only paths, taking the
  top object's paint: hence the (layer, colour, sheet) grouping and filled-shapes-only rule.
- **Clipping masks:** there is no single "apply clip" command. Filled shapes are intersected with
  the clip using `vectorcraft_pathops::boolean`; open cut lines are clipped after flattening in
  `cutcraft-path`. Preserve nested clip intersections and winding rules. Carry clip regions to
  planning; clipping an open line never adds a cut along the clip boundary or closes the line.
- Geometry is stored in absolute page points (y down); mm = pt × 25.4 / 72. Symbol instances and
  images carry their own transform: expand symbols first.
- Read every fill and stroke on an object (objects can have several), not only the top one.
- Retain original stacking order until visibility-dependent operations finish. Outline extraction,
  colour assignment, clipping and same-object fill/stroke combination precede cross-object unions.
  Keep source geometry available for recomputation after a sheet, exclusion or operation change.

### 7.2 Toolpaths

- Apply final size, rotation and mirror before flattening so tolerances are in cut-space mm.
  Target maximum curve deviation 0.025 mm before quantization; budget quantization error
  separately (nominal device step 0.05 mm, to verify). Round absolute positions so errors don't
  accumulate. Test topology as well as distance; detect features that collapse at device resolution.
- **Blade compensation, one method per material** (never both):
  - *software*: account for blade orientation, corners and lead-in; disable machine compensation
    with `FC0` (pen-verified on the original Cameo, 2026-10-23);
  - *machine*: use the verified firmware setting and include its measured clearance in bounds.
  Choose the shipping default only after comparing test cuts on the actual tool. A 0.25 mm offset
  is a calibration candidate, not an established ratchet-blade value. Pen mode uses neither.
- **Overcut** is for closed blade contours only, before compensation. A 0.5 mm value is a test
  candidate; bound it relative to contour length and validate on small shapes. Never extend an
  open cut line automatically. Select a start point that avoids tiny features; smooth closed
  contours need a deterministic fallback when no sharp corner exists.
- **Order:** a containment tree puts holes and nested islands before their enclosing release cut.
  Within those constraints minimise travel with deterministic tie-breaking. Repeat each contour
  as required before its enclosing contour can release it; generated weeding cuts follow artwork.
  Roll planning minimises feed reversals without violating containment; strictly monotonic motion
  cannot be promised for closed contours. Roll mode remains disabled until its own hardware tests pass.
- Validate finite coordinates, positive sizes, open/closed roles, winding, self-intersections and
  degenerate segments. Unsupported geometry fails explicitly. Quantization removes zero-length
  moves and detects newly collapsed holes or duplicate contours; it must not silently change a
  filled piece into an open line. Re-check bounds including travel, aids and lead-ins afterwards.
- Coordinates in mm internally; device units only in `cutcraft-device`.

### 7.3 Cameo driver (clean room)

CutCraft is MIT OR Apache-2.0. The driver is written from public descriptions of GP-GL and observed
Cameo behaviour. **No code from GPL projects** (inkscape-silhouette and others) is copied; facts
learned from them are restated and cited in the driver's docs.

| Item                 | Working hypothesis (public behaviour; **verify on the machine**)               |
|----------------------|--------------------------------------------------------------------------------|
| USB                  | **Observed 2026-10-14:** vendor 0x0B4D, product 0x1121; USB 1.x full speed; serial number but no manufacturer/product strings; interface 0 Printer class, bidirectional; bulk OUT 0x01, bulk IN 0x82, plus interrupt IN 0x83 (purpose unknown); no kernel driver bound on NixOS. Other distributions bind `usblp`: connecting detaches it from the Cameo's interface and hands it back on disconnect |
| Framing              | ASCII commands ended by 0x03                                                   |
| Initialise / status  | ESC 0x04 remains a hypothesis. **Observed 2026-10-18:** ESC 0x05 → `0` with mat loaded and stationary, `2` after confirmed unload. **Observed 2026-10-20 (timing plot):** `1` while moving and `0` whenever not moving, including mid-job while waiting for commands and for about 1 s after receiving commands when idle, before it starts. So `0` after the last byte is not completion until it holds (the driver requires 1.5 s). |
| Firmware             | **Observed 2026-10-17:** `FG` → "CAMEO V1.20" (about 0.7 s); idle status `0`  |
| Units                | 20 per mm; coordinates are **Y (feed) first**                                  |
| Move / cut           | `M y,x` tool up, `D y,x` tool down; curves sent as lines                      |
| Settings             | Force `FX` 1–33, speed `!` 1–10, media `FW`, blade offset `FC`, corners `FF`/`FE`. The cutter keeps the last `FC` across jobs, USB reconnects and a power cycle, so CutCraft sends `FC0` when compensation is off or in software (pen-verified 2026-10-23) and `FC<offset in device units>` for machine compensation (about 0.05 mm of corner loop per unit with a pen; blade behaviour unverified) |
| Cutting area         | ≈ 304 mm wide with a 9 mm left margin, up to ≈ 3 m long (from the model table) |
| Mat vs roll          | Chosen with the button on the Cameo 1; CutCraft enforces software bounds; firmware area setup awaits probes |
| Cancel               | No confirmed cancel command; stop submitting. **Measured 2026-10-20:** the Cameo finishes what it holds (a Stop at speed 1 drew about 3 s more), so the driver's lead bounds the overrun |
| Timing               | **Measured with a pen (2026-10-20, refit with `FC0` on 2026-10-23):** cutting 9.3 mm/s per speed step; tool-up travel about 100 mm/s at any speed; 0.005 s per radian of turn; 0.15 s per lift; 8 ms per point; about 1 s start-up after idling. Chosen so estimates are never short on lettering-like work (pacing must not run ahead); nonzero `FC` adds about 0.1 s per radian, not modelled. Estimates, pacing and the simulator use these |
| End of job / unload  | Uncertain (`SO0`, `H`, `FO0`): disabled until the chosen sequence is verified  |

**Whole-stream checking (2026-10-23, software verified; no new hardware acceptance):**
`gpgl::validate` checks all bytes before `driver::run` accesses the transport, including
hand-built hardware jobs. Only ETX-framed printable ASCII, boundary-aligned ESC EOT/ENQ,
`M`/`D` coordinate pairs inside the software/model area, one force (1–33), one speed (1–10),
one `FC` value (0–200, a conservative validation ceiling, not a measured firmware maximum),
and argument-free `FG` are supported. Missing terminators, overlong commands, invalid
arguments and unknown commands refuse the entire job with `Outcome::Refused`, nothing sent.
The encoder checks its output too; the raw timing sender uses the same check. Refusal appears
in shared UI/MCP status and the cut log, without marking a sheet partially cut.

**2026-10-23 orientation hold:** FN and TB commands are now refused even under the old
experiment permissions, following an off-paper physical result. The original orientation
and portrait CLI cases refuse before USB access. The simulator models an inferred FN0 origin
rebase as a conservative hypothesis. The orientation and setup experiments were withdrawn,
and their CLI commands removed, in 1.0. The candidate list below records the original Stage 1
scope and does not override this hold.

Candidate settings are separate typed **probe-only** permissions: `FE0,0`/`FE1,0`,
`FF0,0,0`, `FY0`/`FY1`, `FN0`, `TB50,0`, `L0`, and bounded `\`/`Z` pairs. Each experiment
permits only its own setting family. Neither ordinary project data nor the app enables them.
`FB`, `TB72`, firmware S-records, homing and origin-changing commands are absent from the
production allowlist. Protocol facts for these candidates are reported in
[the pinned public protocol notes](https://github.com/fablabnbg/inkscape-silhouette/blob/f376a0836b53826e57f50d7ec44658663e629f05/Commands.md);
this is an independent implementation, with no GPL code copied.

The simulator parses and retains candidate setup settings (also `FU`, `H`, `SO0`) without
claiming their geometric effects. Optional track-enhancing and alignment delays default to
zero and remain hypotheses. Probe jobs declare a setup allowance before their first move:
it counts in estimates, blocks further submission during setup, suppresses Ready catch-up,
and delays completion/Stop idle detection past the allowance. A synthetic 6–8 second Ready
interval is covered; a 1.5-second Ready hold alone is insufficient. An allowance is not a
measured upper bound for the real machine. Production headers, compensation defaults and
track-enhancing policy stay unchanged until controlled hardware probes pass.

- **Observed firmware-query transient (2026-10-18):** with the mat confirmed out, ESC ENQ
  returned `2` + ETX before `FG`, then `0` + ETX at approximately 0, 0.5 and 1.0 s after
  the firmware reply, returning to `2` from 1.5 s through 5.5 s. No unsolicited extra reply
  was found in a separate bounded read. An immediate status after `FG` is not reliable load
  evidence; this is one observed timing sequence, not a guaranteed settling deadline. No
  motion commands were sent during this observation.
- **Start preflight (settle rule):** before sending anything, the driver sends only ESC ENQ,
  every 0.5 s, until status has said `0` without a break for 3 s (twice the observed false-Ready
  window). `2` twice in a row means nothing is loaded and the sheet is not started; `1` or a
  single `2` restarts the 3 s; no settled `0` within 15 s ends the start as Interrupted with
  nothing sent; Stop during the wait sends nothing. The connect step's status reply is not
  used as load evidence. The Simulated Cameo models the transient (`0` for 1.25 s after `FG`,
  whatever is loaded) and answers `FG` on connect, so this rule has regression coverage. It
  still needs confirming on the machine (load/unload around a real start) before ordinary
  real-device cutting is enabled.

- **Model table:** vendor/product, verified firmware, endpoint/interface discovery, usable area and
  origin per load mode, ranges, compensation behaviour, safe-stop/checkpoint behaviour, status
  meanings and validated capabilities. Implement only the confirmed model initially. Record
  later-model IDs without enabling motion for them.
- **Before motion-driver work:** a small `cutcraft-cli probe` run on the development desktop
  records USB descriptors, firmware and status replies with bounded timeouts, and feeds the
  simulator ([§7.4](#74-cameo-simulator)). Validate that its query sequence does not move, reset or
  unload the machine; discovery alone must not send guessed commands. Repeat it on the Mac mini
  with the Cameo attached, to check macOS USB access, before the installable Mac build (M4).
- Frame ASCII commands completely, ending each with ETX. Chunk boundaries occur between commands.
  Define read framing, timeouts, retry rules and maximum outstanding transfers. Small byte chunks
  alone do not bound stopping time: one short command can describe a long, slow move. Limit queued
  motion by estimated execution duration as well as bytes, then measure worst-case Stop latency.
- Distinguish queued, transferred and executed data. A failed or cancelled USB transfer may be
  partly delivered; never replay the whole chunk automatically. An uncertain stream position
  enters Interrupted/Unknown and requires the validated recovery sequence and a fresh cut.
- Record actual Stop/Pause latency for long straight cuts, dense curves, slow speed and repeated
  cuts. Initial target: physical stopping within 2 s of Stop under the supported workload. This
  target is unverified; if it cannot be met, revise the advertised behaviour and queue strategy
  before release. UI acknowledgement remains immediate regardless of drain time.
- **Known risk:** the original Cameo connects unreliably on USB 3 ports. Test with the exact cable,
  adapter or hub they will use; a USB 2 hub is the usual fix.

### 7.4 Cameo simulator

Development and testing run against a **simulated Cameo** (`cutcraft-sim`) until the hardware
milestones. The app, CLI, MCP server and tests can then exercise cutting without a machine, and
physical cutting is validated at the end ([§11](#11-milestones)). The simulator replaces the
earlier "recording fake transport": that fake is now the simulator's simplest configuration.

**What it models.** It sits behind the same transport trait as the `nusb` USB transport
([§6](#6-architecture)), so the driver cannot tell it from a real device.

- **Protocol:** it parses the GP-GL bytes the driver sends, checks ETX framing, and rejects unknown
  or malformed commands loudly. It never ignores them. Its command set is the model table's,
  so a command the driver isn't allowed to send fails in simulation too.
- **Machine state:** media loaded or not (mat or roll, chosen with the simulated load button),
  tool position, tool up/down, force, speed, blade-offset and corner settings. Cutting area and
  origin come from the model table, and a move outside them is a simulated fault. Each move is
  recorded. Status replies follow the hypotheses in [§7.3](#73-cameo-driver-clean-room) until real
  replies replace them.
- **Timing:** commands enter a bounded buffer and execute at a speed-derived rate. Status reports
  "moving" until the buffer drains. Pause, Stop, queue-drain detection, progress estimates and
  Stop latency are therefore testable, though only as modelled. Tests run on a virtual clock, so
  they are fast and deterministic; interactive use runs in real time.
- **Faults,** scripted per scenario: disconnect mid-transfer, partial writes, timeouts after
  possible delivery, stalled endpoints, busy and no-media replies, delayed or stale status,
  reconnection, power-off while cutting.
- **Output:** the **virtual mat**, every blade-down path as SVG or PNG, with travel moves
  available as an overlay. Repeated passes over the same contour are counted.

**Where it is used.**

- **Tests:** the preview/send parity case ([§9.1](#91-required-acceptance-cases)) compares the
  virtual mat with the plan the Cut Preview showed: same contours, order, transforms, mirror and
  repeats, within the quantization budget ([§7.2](#72-toolpaths)). Driver state-machine tests
  ([§5.5](#55-cutting)) run against fault scenarios.
- **The app:** development builds offer **Simulated Cameo** as a device, with a live view of the
  blade moving over the virtual mat. The whole cutting workflow can be clicked through, including
  loading prompts, Pause, Stop, interruption and *Cut again*. It is compiled only with
  the `dev-automation` feature, never shown in the end user's release build.
- **Automation:** fake device sessions ([§6.2.2](#622-authority-classes)) use the simulator, so an
  agent can drive the cutting screens over MCP. Against real hardware an agent cuts as they would.
- **CLI:** `cutcraft-cli simulate <project|cutter file> [--scenario NAME] --out mat.svg`
  runs a plan through the driver and simulator and writes the virtual mat and a timing report.

**Calibration and limits.**

- The simulator encodes our assumptions about the machine. It shows that CutCraft sends what it
  intends, and it exercises the state machine, but it cannot establish physical firmware
  behaviour: replies, buffering, stopping distance, accuracy or cut quality. The hardware
  procedure ([§9](#9-testing)) still decides those.
- Each simulator behaviour records its source: *hypothesis* (public descriptions), *observed*
  (a recorded capture on the real Cameo) or *measured* (a timed or dimensional test). Reports
  and test names show which, so a passing simulation of a hypothesis is never presented as
  validated behaviour.
- The read-only probe ([§7.3](#73-cameo-driver-clean-room)) runs early on the development desktop
  and costs no material. Its recorded replies (status codes, firmware string, load states, the
  interrupt endpoint's behaviour) replace the simulator's hypotheses before driver work
  depends on them. Later pen and scrap tests add measured buffer and timing figures (the timing
  plot did, 2026-10-20: §7.3).
- **[later]** On Linux, present the simulator as a virtual USB device (0b4d:1121 through the USB
  gadget subsystem) so the real `nusb` transport, permissions and descriptor discovery are
  exercised as well.

## 8. Platforms and packaging

- **macOS:** one universal `.app` (Intel + Apple silicon), ad-hoc signed (required on Apple
  silicon), initially not sandboxed. Develop and check on the Mac mini: the Intel slice under
  Rosetta, the Apple silicon slice natively, installed-font discovery, and USB access with the
  Cameo. Rosetta does not reproduce an Intel GPU or an older macOS, so the deployment target comes
  from their macOS version, and their MacBook confirms the renderer, fonts and USB at acceptance (M5).
  - Installing: quarantine/Gatekeeper behaviour depends on the delivery method. Test the actual
    download/copy route on a clean user account. Ship a one-page guide using the supported
    **System Settings → Privacy & Security → Open Anyway** flow where needed; command-line
    quarantine removal is a troubleshooting option, not the expected novice installation path.
  - Developer ID signing and notarisation are an optional distribution improvement. They do not
    implement automatic updates; an updater is a separate feature. Initially support manual
    replacement, preserve projects/settings and retain a known-good build for rollback.
- `cutcraft-cli` ships inside the macOS `.app` (`Contents/MacOS/cutcraft-cli`) and beside the app
  elsewhere, so an MCP client can use an installed release without a build. Development-only
  automation (`dev-automation`, [§6.2.2](#622-authority-classes)) is off in release builds.
- **Linux:** packaged in `~/nix-config`; udev rule (`TAG+="uaccess"` for 0b4d:1121); detach or
  blacklist the `usblp` printer driver if it claims the Cameo.
- **Windows (nice to have):** check the app in a Windows VM against the simulator. Cutting needs USB
  passthrough to the VM and a WinUSB driver for the Cameo (Zadig) or another route; decide
  before starting.
- Development dependencies in `shell.nix` with pinned inputs. No CI for now (the developer, 2026-10-20:
  a one-time install for one user, no releases to support): the required checks run locally on
  NixOS and on the Mac mini before changes are merged. Each release records app/engine revision, toolchain, minimum OS,
  supported device/firmware, known limitations and third-party notices, including bundled fonts.

## 9. Testing

- **Golden tests** for every VectorCraft command used ([§6.1](#61-vectorcraft-dependency)).
- **Make cut-ready tests** on files written for the project: text, thick strokes, same-colour
  overlaps, letters with holes, script text, fill+stroke, clipped groups, white backgrounds,
  duplicates, two layers, nested symbols, multiple appearances, transformed/dashed strokes,
  missing glyphs, page selection and unsupported effects. Their real files are used locally and
  never committed, including generated previews, extracted fonts or converted copies.
- **Toolpath and command snapshots:** expected output files checked in.
- **Simulator** ([§7.4](#74-cameo-simulator)) scenarios test Pause, Stop and interruptions without
  hardware: busy and no-media replies, delayed/stale replies, partial writes, timeout after
  possible delivery, stalled endpoints, reconnect, app shutdown and duplicate starts. The virtual
  mat checks what was sent against the plan. The simulator tests CutCraft and its state machine; it
  cannot establish physical firmware behaviour.
- **Automation tests** ([§6.2](#62-automation-and-mcp)):
  - a property/fuzz test of MCP and control-protocol input;
  - golden transcripts for each tool;
  - a schema snapshot, so tool and command schema changes are deliberate;
  - a parity sweep that sends every command through every origin: `command_run`, batch steps and
    `$N` references, `engine.execute`, `ui.menu.invoke`, injected pointer and key input, and
    `cutcraft-cli run`. It asserts that each gives the same result as their own input, and that
    motion reaches the simulator only after the same preconditions.
- **Hardware procedure** for driver changes: verified query-only probe, motion with the blade
  removed (an air pass with draw commands, since tool-up moves alone can leave the Cameo still),
  pen on paper, scrap vinyl, then real material. Record model/firmware, OS, cable/hub,
  material/tool, settings, dimensions and stop latency. Verify both maximum travel and small detail.

### 9.1 Required acceptance cases

| Case | Pass condition |
|------|----------------|
| Physical scale | A 25.4 mm fixture imports at 25.4 mm in each supported format; unit/viewBox ambiguity prompts a choice, never silent resizing |
| Preparation replay | The same source and recipe give identical normalized geometry/report; Undo and reapply restore it; cancellation leaves the last complete result |
| Text and holes | Embedded/substituted/missing-glyph cases take the correct route; holes and nested islands survive outlining, clipping, unions and mirroring |
| White and excluded art | White alone never excludes artwork; a page-sized background candidate needs a Cut it/Don't cut answer; Undo and source exclusions regenerate affected welds |
| Row identity | Grouping changes preserve exclusions; duplicate names/colours cannot silently change object exclusions or material settings |
| Layered design | Colour pieces retain assembly positions through placement, common resizing, mirroring and marks; verify a two-colour scrap assembly |
| Bounds | A design that fits before compensation but exceeds bounds afterwards is blocked by both UI and CLI; includes test cuts, weeding, travel and device rounding |
| Preview/send parity | The encoded plan has the same operation order, transforms and settings reviewed in Preview, and the simulator's virtual mat matches it within the quantization budget; any relevant edit makes it stale |
| Duplicate/repeated cuts | Equivalent duplicate contours within one operation cut once; intentional repeats and independent sheets retain their cuts |
| Save/reopen | A project repeats with the source missing and fonts/presets changed; accepting a source update produces an explicit diff; an interrupted save preserves a readable version |
| Interruption | Partial write/disconnect never auto-replays motion, claims blade-up or marks an uncertain sheet complete; reconnect requires a fresh start |
| Physical quality | A pen test verifies origin, axes and dimensions; scrap vinyl verifies holes, corners and seam closure with the selected compensation method |
| Automation reach | Every automation origin can run every command, cutting included, under the same preconditions as the end user's input; a remote edit during a cut is refused as theirs is deferred; automation actions are marked with their origin; writes onto a source design fail, and writes outside a configured write root fail |
| Automation parity | The same design and steps through the window, `cutcraft-cli run` and MCP give the same recipe, sheet, plan id and preview |
| User workflow | The end user opens, sizes, prepares, previews, tests and cuts a representative project, then reopens it for a repeat, using only the shipped prompts/guide |

Choose and record physical dimensional tolerances during calibration; device step size is not an
accuracy guarantee. Use geometry/property assertions alongside snapshots: a visually plausible
snapshot can still contain duplicate cuts, lost holes or wrong physical scale.

### 9.2 Local project corpus

The user supplied a local archive of previous projects during draft 3 review. Configure its location through
`CUTCRAFT_CORPUS_DIR`; don't commit the archive or a machine-specific absolute path. Keep a local
manifest of relative paths, content hashes, expected size/colours and approved exclusions. Public
CI uses authored fixtures; local acceptance runs use selected real projects.

Sample across live-text/outlined pairs, embedded and unembedded fonts, text on paths, white artwork,
named layers, stroke-heavy drawings, tiny distressed details, garment mockups, multi-colour designs,
front/back/sleeve variants and original/CUT versions. A reference image is evidence of visual intent,
not automatically the geometry or material that should be cut. Do not select the latest-looking
filename as an authoritative version without comparison. Research notes record the initial sample;
passing an importer query is not yet a successful cut-ready conversion or hardware test.

## 10. Out of scope for v1

- Print and cut (registration marks read by the optical sensor). Check whether the end user uses it
  ([§12](#12-open-questions)).
- Splitting designs bigger than the mat or roll across several sheets (tiling); automatic nesting
  or independently packing pieces. **Move to corner** preserves arrangement.
- Bluetooth; Cameo 2 and later; other brands (the model table keeps the door open).
- Design editing beyond arranging (use VectorCraft).
- Silhouette Studio's `.studio3` format.
- Any automation route that starts, resumes or repeats motion ([§6.2](#62-automation-and-mcp));
  MCP reachable beyond loopback.
- Automatic updater; mixed tools/materials in one loaded sheet; editing nodes, text or design
  content. Explicit conversion choices and arrangement remain in scope.

**v1 (M0–M6)** is a complete vector-to-vinyl/HTV workflow on the end user's confirmed Cameo and MacBook,
including save/repeat, material editing, test cut, interruption handling, validated roll mode,
weed lines, layering marks and watch/reload. M5 establishes the mat-based workflow before M6 adds
the remaining v1 capabilities. **[later]** Tracing, visibility subtraction, overlap, pen/cardstock
workflows and additional import formats/platform packages have separate gates. Unsupported or
deferred actions are absent or clearly explained.

## 11. Milestones

| Milestone | Deliverable and exit gate | Needs |
|-----------|---------------------------|-------|
| M0 | Workspace, pinned dependencies, fmt/clippy/tests; minimal egui app runs on NixOS and the Mac mini (both architectures); CI green on all runners (dropped 2026-10-20); query-only USB probe on the desktop records device/firmware and load states | Desktop + Mac mini + Cameo (query only) |
| M1 | `prepare`: selected corpus imports with size/font/appearance diagnostics; deterministic preparation recipe, provenance and golden tests; no unresolved silent omissions; PNG/JPEG design tracing | Local corpus; M0 engine pin |
| M2 | `plan`: shared domain types, sheets, immutable validated plan, SVG preview, project save/reopen, bounds/geometry tests; Cameo simulator with the virtual mat and parity test; command registry with schemas and authority classes; headless MCP and `run` ([§6.2.9](#629-scope)) | M1 |
| M3 | App: preparation decisions/Undo, colour/layer grouping, assembly view, sizing, preview, save/repeat and material adjustment; main workflow usability check; control protocol, UI tools and Assistant connected badge; full cutting workflow against the Simulated Cameo | M2 + Mac mini |
| M4 | Validated motion/pen driver on the desktop, then macOS USB on the Mac mini; installable Mac build; measured origin/scale, queue-drain, Pause/Stop and interruption behaviour, fed back into the simulator; device status/stop tools, fake device sessions and the automation boundary acceptance case | M0 device gate + M3 + Cameo |
| M5 | Mat-workflow acceptance: scrap then real vinyl/HTV, test cut, repeat project; then install on the end user's MacBook and their independent user trial; bring basic marks forward for layered trial projects | M3 + M4 + Cameo; the end user's MacBook for the final step only |
| M6 | v1 release: validated roll mode, weed lines, layering marks, watch/reload and full material editor; all v1 acceptance cases pass on representative projects | M5 + feature-specific hardware evidence |
| M7 | Everything tagged **[later]**: tracing embedded pictures, remaining picture and import formats, visibility subtraction, overlap, gradient/pattern conversions, independent rearrangement, object-level update matching, schema migration, screen-reader support, unload button, pen/cardstock workflows; virtual USB simulator; Linux package/udev and optional Windows (installer scripts exist since 2026-10-20, untested on their target systems; a Windows Cameo still needs a WinUSB driver) | Prior stages; per-platform/material validation |

Until M4, cutting is developed against the simulator; physical cutting is validated at the end,
on the desktop first. The read-only probe (M0) costs no material and comes early so the simulator
copies recorded replies, not guesses. The end user's MacBook is needed once for its macOS version (before
fixing the deployment target) and once at M5. M2 defines persistence before the UI
depends on it. Add overlap only after the simpler layered workflow succeeds; it is not a v1 gate.

## 12. Open questions

- **The end user's MacBook:** macOS version and model year (they can read them from About This Mac or send a
  screenshot), and the ports/adapter they'll use. These set the deployment target and the USB setup
  to reproduce on the desktop and Mac mini; do not infer ports from the year alone.
- **How the end user works:** multi-colour layered vinyl? Iron-on? Each colour on its own layer, or only
  coloured shapes? Do they use print and cut? Do they rely on red hairlines as cut lines?
- **Typical project:** archive supplied; select the canonical versions and intended physical sizes,
  materials, colours and pieces to exclude. Confirm which examples were actually cut versus printed
  or kept as mockups. The existing `.studio3` files remain unsupported unless scope is revisited.
- **Acceptance corpus:** which simple, detailed, layered and roll projects represent success, and
  which need layering marks during the earlier M5 mat-workflow trial.
- **Windows:** worth doing, given the driver step?
- **Silhouette libraries (decided 2026-10-17):** `.studio3` stays unsupported for now. User-made
  files are not encrypted (they begin `silhouette05;` and contain zlib streams), but reading them
  would need independent reverse engineering, outside the clean-room rule of citing public
  descriptions. Revisit only with a public description or a licence review.
- **Automation on the end user's MacBook:** the loopback MCP service is on by default in every build
  ([§5.10](#510-ai-control-through-mcp)). Should their release build keep it on, or ship it off with
  the setting available? Remote help from another machine needs an agent running on their Mac or
  an SSH tunnel to the port, which they must be able to manage without calling for help. The
  control port ([§6.2.6](#626-control-protocol)) stays off unless launched with `--control`.

## 13. Revision history

**Draft 3.6:** GIF (one chosen frame of an animation) and BMP join PNG and JPEG for tracing.
Records that Silhouette `.studio3` has no public specification and Cricut Design Space has no
exportable project file, with guidance for both and an open question on Silhouette libraries.

**Draft 3.5:** PNG and JPEG designs are traced in v1 (M1), because many supplied designs exist
only as pictures. Other picture formats and tracing pictures embedded in vector files remain
**[later]**; embedded pictures need explicit exclusion in v1. Photo-like pictures, such as shirt
mockups, get a warning and are excluded from corpus acceptance.

**Draft 3.10:** records the hardware phase and decisions since 3.9. One sheet only, every colour
and layer ticked by default, one material per cut (§4.1, §5.4). Strokes on filled shapes are
part of the shape by default, with a design-wide override. Layering marks are four inward
triangles (provisional until the user's workflow is known); weed lines can be inverted.
Repeated test cuts keep clear of each other. Real-Cameo mat cutting is enabled: the pen sheet
measured scale and origin within 1 mm and macOS USB cut from the Mac mini (2026-10-19); roll
cutting stays gated. The timing plot measured the status meaning, Stop drain and timing model
(§7.3), now used by estimates, pacing, the completion check and the simulator. No CI for now;
installer scripts for each platform (§8, M7). The end user's MacBook steps wait for their return.

**Draft 3.9:** records M3 decisions. Cutting runs through registry commands: `device.connect`
(the Simulated Cameo, [§6.2.2](#622-authority-classes)), `cut.sheet` / `cut.test` choose a job
and return its loading steps, `cut.start` confirms the material is loaded, then `cut.pause`,
`cut.resume`, `cut.stop` and `cut.dismiss`; `device.status` and `device.wait`
read the state. Material settings are adjusted inline; test cuts never change or certify them. A sheet's plan is checked again at `cut.start` and a changed plan is not sent.
The encoder splits long moves so no command holds more than about 0.5 s of estimated motion, and
the driver keeps at most 1.5 s queued, so Stop is bounded by time rather than bytes (modelled;
to be measured, [§7.3](#73-cameo-driver-clean-room)). New window and automation commands:
`prepare.run` (the Make cut-ready button), `design.size`, `rows.group`, `sheet.adjust`,
`materials.save` / `materials.delete` (their library, beside the settings), `sheet.place
{corner}`, `project.update` (*Update from design*), `view.show` / `view.inspect` (MCP `ui_show`,
`ui_inspect`) and `render_preview` view `assembly`. Make cut-ready messages give sizes in inches
first, with millimetres after. The control protocol's `ui.inspect` reports `pixels_per_point`,
since screenshots are in pixels and `ui.pointer` takes window points.

**Draft 3.8:** drops MCP logging (`logging/setLevel`, `notifications/message`), which the
protocol is removing (SEP-2577); the session journal records what ran.

**Draft 3.7:** automation can do anything the end user can (the developer's decision). The rule that automation
never moves the cutter is replaced by parity: cutting, test cuts, pause/resume, unload and
material confirmation are available to MCP, the control protocol and `cutcraft-cli run` under the
same preconditions as their clicks, marked with their origin. The `human-only` class becomes
`motion`. Automation roots become optional limits; without them automation reads and writes
files as widely as they can. Writes still never target a source design.

**Draft 3.4:** reconciles the requested default-on MCP service, built on the `m0` branch, with
drafts 3.2–3.3. Adds [§5.10](#510-ai-control-through-mcp): the running app serves Streamable HTTP
MCP on loopback by default, with saved on/off/port settings, Host/Origin checks, a stdio bridge
and client setup help. The live service replaces the token-authenticated remote MCP mode; the
control protocol remains for injected UI input, off by default and token-authenticated.
`cutcraft-control` takes the command registry role planned for `cutcraft-command`. The interim
`cutcraft_*` tools move to the [§6.2.4](#624-tools) names with the M2 registry. Authority classes
and the rule that automation never moves the cutter are unchanged. These capabilities do not
imply preparation or motion is implemented.

**Draft 3.3:** development moves off the end user's MacBook. NixOS desktop and Mac mini are the development
machines, a Windows VM is optional, and their MacBook is needed only for its macOS version and final
acceptance. Adds the Cameo simulator ([§7.4](#74-cameo-simulator)), which replaces the recording
fake, so cutting is developed without hardware and validated physically at the end. Records the
confirmed Cameo model and its observed USB descriptors. Reworks the M0, M2–M5 gates accordingly.

**Draft 3.2:** adds automation and MCP ([§6.2](#62-automation-and-mcp)), modelled on the sibling
Craft apps: every action a command, a headless and remote MCP server, a token-authenticated
control protocol, automation roots, and a batch CLI. New for CutCraft: authority classes, under
which automation never moves the cutter; generated parameter schemas; revision checks against
concurrent edits; and progress/cancellation designed in from the start. Sources: VectorCraft
`docs/mcp.md` and `docs/control-protocol.md` at `540505fc`; GridCraft `docs/mcp.md` and
`docs/control-protocol.md` and PhotoCraft `docs/mcp.md` and `docs/control-protocol.md` as of
2026-10-14; [MCP specification](https://modelcontextprotocol.io/specification/2025-06-18) revision
`2025-06-18`.

**Draft 3.1:** explicit v1/later scope, a required answer for suspected backgrounds, simpler v1
source-update matching, and local-project evidence. Reconciled milestones and acceptance cases
with those decisions. Technical review evidence is retained below.

**Draft 3:** preserves the cutting-tool scope and replaces assumptions with explicit decisions and
acceptance gates. Main changes: reversible preparation; conservative exclusions; physical sizing;
appearance/font diagnostics; assembly-preserving placement; self-contained saved projects;
validated immutable plans; honest cutter states; earlier Mac/USB checks; measurable release tests.

- Engine review used the local VectorCraft checkout at `540505fc`, the revision cited in the prior
  research. `crates/engine/src/cmd/fileio/load.rs` shows that page selection bypasses AI editing-data
  recovery and exposes `preview_only`/warnings. `cmd/fonts.rs` reports substituted styles and missing
  glyphs in addition to missing families. These are revision-specific contracts to test, not promises
  about every future engine version.
- Local archive inventory, sample metadata, import queries and reference images informed the
  requirements (the notes stay private with the archive). The installed CLI used
  for those queries was not established to be the pinned engine revision. Results are exploratory,
  and the acceptance suite must rerun them against the selected dependency.
- [SVG coordinate systems](https://www.w3.org/TR/SVG2/coords.html) and
  [CSS absolute lengths](https://www.w3.org/TR/css-values-4/#absolute-lengths) establish the
  distinction between user coordinates, physical units and CSS pixels.
- [nusb endpoint documentation](https://docs.rs/nusb/0.2.7/nusb/struct.Endpoint.html) documents
  asynchronous cancellation and partially completed transfers. USB completion/cancellation alone
  cannot establish the state of buffered mechanical motion; the hardware tests must do that.
- [Cargo's lockfile guidance](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html)
  supports recording the application's resolved dependencies for repeatable builds.
- [Apple's opening-app guidance](https://support.apple.com/en-us/102445) and
  [notarisation documentation](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
  inform installation testing. Signing/notarisation and automatic update delivery are separate work.

Neither the draft 3 review nor the earlier driver research verifies commands, stopping latency,
blade offsets or material settings on the user's machine. Performance/latency numbers in this
document are proposed acceptance targets until measurements are recorded.
