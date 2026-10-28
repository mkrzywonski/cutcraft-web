# Intentional port differences and deferred features

Status: **proposed with spec version 2**, not a waiver signed by the developer. Existing web-v1
architecture decisions are identified separately from new decisions awaiting approval.
This register is part of [the spec](spec.md). All unlisted supported reference behaviours carry
forward through [the detailed contract](cutcraft-ui.md). “Not built yet” is never an exception.

## Adaptations required by the already chosen web architecture

| ID | Native feature | Web replacement and boundary | Authority / requirement |
|---|---|---|---|
| A01 | Native Open/Save dialogs and filesystem paths | Browser picker/drop/uploads; library Save/Save As; explicit Download. First Save still asks folder/name. Cancel writes nothing. No arbitrary paths from either browser or MCP. | Web-v1 §§6,9–11; OPEN-01, SAVE-01 |
| A02 | Disk watching, changed/missing local file indicators | Explicit Update from Design upload with retained settings/diff/Undo. No claim that the service watches the laptop. Recent entries use owned IDs, not inaccessible local paths. | Web-v1 §10; SAVE-03 |
| A03 | Installed client OS fonts | Bundled plus per-person uploaded fonts; same substitution/missing-glyph safeguards. No server-wide cross-person font cache leakage. | Web-v1 §10; OPEN-10 |
| A04 | USB on the user's computer; platform USB drivers | Server cutter picker, turn/queue and USB permissions; no WebUSB/client USB. Keep original Cameo model and driver guards. | Web-v1 §§3,14,25; CUT-04, CUT-08 |
| A05 | Quit/sleep/window-close guard | No Quit that shuts down the shared service. Closing/reloading a tab detaches the viewer; accepted cuts remain supervised. Reconnect observes, never restarts. Warn about unsaved work where browsers permit; draft recovery does not depend on unload events. | Web-v1 §§7,14; SAVE-01, CUT-06 |
| A06 | Full source path in title; Show log in folder | Show authorised design/project name and library location/reference; copy link/reference and download logs. No server paths exposed. | Web-v1 §6; UI-04, HELP-02 |
| A07 | One window's preferences and native automation roots | Units/snap in project; theme/font/order/locks/defaults/libraries/fonts per person; zoom/pan/folds/layout override per browser; desktop screen shared per session. Owned-ID authority replaces optional unrestricted roots. | Web-v1 §§6–8; PANEL-03 |
| A08 | Leave mat loaded after a test and for the design | **Approved 2026-10-28:** cutter-owned retained-load context for the same actor/cutter/mat generation, exact occupied areas and explicit Start each time. Unload, disconnect, Stop, changing users, uncertain motion or mismatched setup requires fresh loading. Recovered drafts cannot prove physical loading. Requires safety acceptance before enabling. | CUT-07; web §23 Q2 |
| A09 | Default-on per-window MCP, loopback port/address controls, port competition | One service HTTPS MCP address and per-person keys/settings. No per-tab listener/port picker, optional unauthenticated LAN key mode, or second hidden design session. Human key administration remains. | Web-v1 §§6,15,17; AUTO-03 |
| A10 | Native injected input, focus/resize/screenshot; local stdio/headless CLI | Browser test harness input/capture; MCP identifies an attached browser or reports capture unavailable. Service-rendered previews are available separately. No silent headless fallback or browser-quit claim. Network-aware stdio bridge may be supplied in M6; native local-window bridge is not shipped unchanged. | Web-v1 §§5,9,15; AUTO-02; capture adapter needs an M6 contract |
| A11 | macOS/Windows/Linux desktop installers, Gatekeeper/Zadig, X11/DBus dialog fixes | Server Debian packages/image/container; clients need a supported browser. Keep responsive/cancellable file handling and actionable failures, not desktop portal workarounds. | Web-v1 §21; UI-06 |
| A12 | Reserved native shortcuts | Working menus/buttons on every browser; intercept only supported shortcuts without stealing browser/tab/text actions. Compact touch alternative for hover/right-click/drag. | Web-v1 §9; UI-05, VIEW-02 |
| A13 | Native process lifetime for unsaved material tuning | **Approved 2026-10-28:** preserve temporary settings across browser reloads, eviction, sign-out and service restart in the same person’s session. Preset selection replaces them; explicit session close ends carry-over. Preset Save/Save As remains explicit. | MAT-02; web §23 Q1 |
| A14 | Local executing project locked against all edits | Frozen service execution cannot change; another preparation draft can be edited under web revision rules. Active-cut controls/geometry edits cannot silently change the frozen copy. | Web-v1 §14; CUT-05 |
| A15 | Native single-user screen and local availability | Authenticated per-person shared screen/settings/results; ownership/refusal/reconnect UI required. Compact page navigation stays independent. | Web-v1 §§6–9; UI-07, UI-08 |
| A16 | Original no-CI/manual-only desktop workflow | Web development standard defines checks/CI/packaging for the new product. Old cost-saving repository setting is historical, not a UI behaviour to copy into this repo. Enabling/changing actual workflows still follows developer authorisation. | Web development standard §7; old commits82e7f93/2d55489 |
| A17 | Desktop folds saved together with panel arrangement | Retain collapse/attention behaviour, store folds in browser and order/locks per person. Reset clears both scopes; changing browser does not reset shared arrangement. | Web-v1 §8 explicitly supersedes native1.0.3 storage scope |
| A18 | Original standalone offline use | Core workflow needs only the home network/service, not internet or cloud login. No promise to cut/prepare with the service unreachable. | Web-v1 Architecture B; BASE-04 |

## Deliberately removed historical approaches

These were removed before 1.0.5; restoring them would be a regression, not completeness.

| ID | Removed | Replacement |
|---|---|---|
| X01 | Tested/provisional material flags, certification checkbox, material-test wizard, force bracketing/result questions, test prerequisite | Inline settings plus explicit preset Save/Save As; optional single test pattern; Troubleshooting. MAT-01–03, CUT-02 |
| X02 | Multiple sheets by colour, numbered sheet controls/next-colour prompts | One sheet with all rows ticked initially; manual selection for successive colours. BASE-03 |
| X03 | Raw import/preparation JSON in right panel | Human Issues/Info and structured tool responses. BASE-05 |
| X04 | Duplicate panel Cut entry, Cut performing different actions by view, stray Test Cut entry links | Toolbar/menu navigation, separate review/Start on cut screen, contextual Exit. CUT-01–03 |
| X05 | Auto-open material/mat editor on selection; hidden Delete; duplicate defaults in right panel | Select first, explicit visible actions, modal editors, defaults in libraries. MAT-05–07 |
| X06 | Picture-size question for missing DPI; quantized-only photo detection | Calculated size and exact-colour-plus-flatness warning. OPEN-04, PREP-03 |
| X07 | Separate View options group; groups disappearing when switching design views | Cutting aids includes options; ten shared groups. PANEL-01–03 |
| X08 | Withdrawn setup/orientation hardware experiments and guessed commands | Production allowlist; hardware investigation remains separately gated. CUT-08 |
| X09 | Early ban on AI cutting; unrestricted filesystem access for AI | Authorised action parity, owned-ID security and human administration of access. AUTO-01–03 |

## Existing deferred scope, not missing port features

| ID | Not part of this release | Required handling |
|---|---|---|
| D01 | Print-and-cut optical registration, tiling/nesting/independent piece packing, mixed tools/materials | Do not offer nonfunctional controls; preserve assembly placement. |
| D02 | Bluetooth, Cameo 2+ and other brands | Recognise unsupported devices without enabling motion. |
| D03 | `.studio3`, Cricut cloud-library extraction | Explain originals/exports that actually exist; no promise to reverse engineer or modify those libraries. |
| D04 | Embedded-picture tracing; TIFF/WebP/AIT/EMF/WMF/Affinity; PSD layer-preserving tracing | Explicit unsupported/exclude/export guidance; advertise only tested formats. |
| D05 | Visibility subtraction, colour overlap/spread, general gradient/pattern conversion | Explicit solid-colour/exclude choices remain required; no hidden flattening. |
| D06 | Node/text design editing, object-level update matching, arbitrary design duplication/repacking | Whole-design size/place/rotate and unambiguous row matching remain. |
| D07 | Automatic unload, user pen/cardstock workflows, hardware roll cutting | Manual unload; developer pen tests separate. Roll planning/simulation retained, real-roll action disabled pending its own acceptance. Never confuse unavailable hardware with missing preview controls. |
| D08 | Virtual USB gadget simulator; later MCP elicitation/subscriptions/additional prompts | Existing simulator, resources, ordinary progress/cancellation and required workflows remain. |
| D09 | Full screen-reader promise for egui workflow | Record measured gaps; keyboard/focus/labels/numeric alternatives and web accessibility requirements remain mandatory. No claim of accessibility conformance from screenshots. |

## Not exclusions

Material/mat editors, all ten panel groups, source/overlay alignment, Save As, persistent panel
controls, interactive placement/resizing, the cut screens, working Help and the imported issue
examples are mandatory. Complexity, browser latency, absent protocol fields and unfinished M4
code are implementation work, not grounds to drop these features. Original spec promises not
implemented in 1.0.5 are called out in the source map and need tests or an explicit developer-approved
scope amendment; they do not disappear under “reference parity”.
