# Changelog

Each version of CutCraft-Web and what changed in it. Versions are `major.minor.patch`: patch for
fixes and small changes, minor for new features, major for big changes. Raise it with
`scripts/bump-version.sh patch|minor|major`.

## 0.20.1 — 2026-11-12

- Resolve an unknown Start without retrying after a changed cutter fence and a missing receipt. Preserve newer requests during reconciliation, distinguish sign-in failures from outages, and keep project controls working when browser recovery storage is unreadable. Retry explicitly names the original prepared revision.

## 0.20.0 — 2026-11-12

- Update russh to 0.65 to address RUSTSEC-2026-0495; CutCraft uses its SSH client, not the affected agent server.

- Resize designs from all four corners with linked proportions and one Undo, keeping the opposite corner fixed. Double-click a canvas dimension to type a length. Keep the dropped geometry visible while the updated plan arrives. Require browser protocol 8 for anchored sizing.

## 0.19.0 — 2026-11-11

- Draw estimated cutting progress from the sender’s exact byte ranges and quantized paths, with completed/remaining colours, queued motion and an approximate blade marker. Label estimates explicitly, distinguish startup and pausing, and play cut order by distance including repeat passes.

## 0.18.6 — 2026-11-11

- Remember an unresolved Start across browser reloads and keep new Start disabled. Offer read-only status checking and a deliberate retry of the original request after looking up its result; failed lookups never resend motion.

## 0.18.5 — 2026-11-11

- Keep each dialog’s layout independent so Save As remains usable after a save conflict. Apply dark appearance consistently to panels, menus and dialogs.

## 0.18.4 — 2026-11-11

- Restore all eight media resize handles. Keep the opposite edge fixed when resizing, preserve media size when moving, and snap positions to the mat’s printed grid. Show selected dimensions in the chosen units.

## 0.18.3 — 2026-11-11

- Allow native development checks on a fresh checkout without generated browser files. Report the missing desktop bundle explicitly; require current-version desktop assets for release builds.

## 0.18.2 — 2026-11-11

- Update the MCP SDK to 3.9.0 to address unbounded transport memory use.

- Show the mat and material before preparing a cut, clearly marked as not prepared. Draw active test previews at the recorded execution area and reuse their geometry during progress updates.

## 0.18.1 — 2026-11-11

- Mark the cutter’s start corner and loading direction on mat previews. Show a live diagram of the outer mat, sticky grid, rulers and start corner while editing a mat. Give material and mat editors more vertical room.

## 0.18.0 — 2026-11-06

- Re-render the visible artwork region after zooming or panning, at the display pixel density. Keep the previous image while the view settles, coalesce render requests and reject obsolete replies.

## 0.17.2 — 2026-11-06

- Keep the desktop toolbar on one line at narrow widths and large text sizes, with overflow controls in More and a compact view selector. Scale headings, small text and dialog text along with menu text.

## 0.17.1 — 2026-11-05

- Keep same-source artwork visible while a replacement render loads, retaining its correct region. Clear private images and lists across source, session and account changes.
- Format size choices in the selected units, improve selected-toggle contrast, and use singular piece/line counts where appropriate.


## 0.17.0 — 2026-11-05

- Restore the reference physical mat drawing, with rulers on each edge, inch/centimetre markings, custom printed grid origins and approximate-measurement guidance. Both design and frozen cut previews use the saved mat profile.


## 0.16.0 — 2026-11-05

- Recover the owned active-cut preview after a browser reload without starting motion. Use the execution's immutable job, cut kind and material bounds, and refuse previews made across a cutter-state change.
- Show numbered occupied test areas on the matching mat. Hiding the overlay preserves reservations.


## 0.15.2 — 2026-11-05

- Require fresh loading after stopping a test cut, including when completion races Stop. Preserve occupied areas and refuse another Start until a new load is observed.


## 0.15.1 — 2026-11-05

- Keep Stop available through dialogs and text editing; add desktop Save As, Redo and view shortcuts without intercepting text-field Undo.
- Wait for confirmed unloading before allowing another simulated mat load.
- Keep cutter selection consistent and fixed during motion. Release completed prepared previews so another cut can be prepared, preserving queued copies and newer preview responses.


## 0.15.0 — 2026-11-04

- Prepare updated cut settings on the same loaded mat while retaining earlier test areas and the current queue turn. Changes to the physical material/mat require fresh loading. Stale or busy replacements change nothing.
- Disable Start for an outdated prepared copy until the current settings have been reviewed. Match source identity as well as revision; protocol 7 requires older browser clients to reload.
- Expose the same validated operation to AI tools as cut_replace.

## 0.14.4 — 2026-11-04

- Bind loaded-mat reuse and fresh-mat confirmations to the same account, across its browsers. A different or recreated account must load fresh material. Recheck account authority before releasing a recorded Start to the cutter.

## 0.14.3 — 2026-11-04

- Keep comparison-view backgrounds inside their own panes when panning. Scroll to pan, Shift-scroll horizontally, and use modifier-scroll or pinch to zoom around the pointer.

## 0.14.2 — 2026-10-29

- Recover an owned prepared cut after reloading the desktop page without queueing another copy or starting motion. Retain the selected cutter and show when a prepared copy is waiting in its queue.

## 0.14.1 — 2026-10-29

- Keep queued private cut previews visible only to their owner. Waiting cuts no longer request another person’s preview, and preview errors leave the rest of the workspace connected.
- Clear previous action results so an earlier Save cannot close a later dialog.

## 0.14.0 — 2026-10-29

- Add the desktop browser workspace: graphical toolbar, comparison and mat views, editable sheet settings, material/mat dialogs, library saving and simulated cut controls. This independent M4 branch remains under development; see docs/m4-progress.md for outstanding parity work.
- Save unfinished projects without certifying them for cutting. Preserve Undo/Redo through worker restarts and keep uploaded project bytes unchanged.
- Prepare a validated test pattern independently of artwork readiness, preserving prior test areas when planning later cuts.
- Bundle the desktop assets with the server and pin the matching build tools in the development shell.

## 0.13.1 — 2026-10-27

- Return a durable refusal when a mat change or disconnect cancels Start before release; retries and lookup preserve it across restart. Failed receipt writes remain retryable storage errors and never report Started.

## 0.13.0 — 2026-10-27

- Drain active cuts on SIGTERM/Ctrl-C, continue serving Stop and this version’s assets, then checkpoint workers/queues and join device/history threads before restart.
- Show update waiting, disable new Cut actions, pin Linux worker launches to the running executable, and provide a protected reference systemd unit.
- Name the server executable `cutcraft-web`; keep the imported package name and existing data paths compatible. Synchronous workers no longer allocate the server runtime’s thread stacks.
- Enforce the 100-page PDF/AI import budget before building page geometry.

## 0.12.0 — 2026-10-27

- Limit preparation to two resident workers, checkpoint and unload idle/LRU sessions, bound queued requests and upload bodies, and discard superseded previews.
- Add targeted preparation cancellation over correlated worker IPC; protect the last draft before cancellable work and isolate worker memory failures.

## 0.11.0 — 2026-10-27

- Persist queue order, current turn and accepted Queue receipts independently of the library and preparation sessions; replay accepted requests before mutable revision checks.
- Restore waiting jobs without mat/test authorization or runnable state. Main-cut turns already consumed by durable execution records cannot return after a stale queue checkpoint.
- Fence late queue snapshots with their own monotonic revision, and checkpoint explicit queue actions, job updates, Start admission and shutdown.
- Lock the authority directory so a second server cannot claim the same state; reject damaged execution outcomes instead of guessing queue recovery.
- Add restart, duplicate-request, late-snapshot and exclusive-authority regressions.

## 0.10.0 — 2026-10-27

- Persist outstanding Stop intent separately from Start record writes, so Stop can be acknowledged while Start persistence is blocked.
- Recover Stop confirmation across server restart and reconnect without restoring a runnable or inheriting a new actor. Wait for settled status or no media before resolving it; source/history deletion cannot erase the intent.
- Keep terminal Stop receipts so delayed requests cannot resurrect a confirmed Stop, and confirm all outstanding intents captured at startup together after the cutter settles.
- Add simulator regressions for restart, source deletion, no resumed motion, original cut-outcome retention, delayed requests and confirmation batches. Target-Pi latency/load acceptance remains open.

## 0.9.0 — 2026-10-27

- Persist an accepted Start before acknowledging it or releasing the job to the device worker. Lookup shares the acceptance transaction; preparation and disk I/O never hold the cutter state lock.
- Preserve Stop during a blocked record write; cancel release after storage failure, disconnection or an observed mat change.
- Recover accepted results in the cutter service even when the driver never took the job. Restart marks unfinished records interrupted and never resumes motion; deleting ordinary library history cannot delete these results.
- Make execution recording idempotent, retain the first terminal outcome, support imported unversioned records as schema 1, and fail closed on damaged or unsupported records.
- Add simulator regressions for the pre-driver restart window, failed storage, blocked persistence with Stop, and cancellation. Outstanding Stop intent, queue recovery and load/latency acceptance remain M3 work.

## 0.8.0 — 2026-10-27

- Add validated immutable cut copies through the shared `cut.copy` registry command; preparation never queues or moves a cutter.
- Keep each copy and its keyed request receipt atomically in private cutter-service storage. Worker/session recovery replays the same result and refuses changed request bodies.
- Queue, loading details, preview and Start resolve the copy ID at revision 1; design edits, later library revisions and closing the design cannot retarget it. Original artwork/project downloads remain private.
- Add MCP `design_status` and `design_cut_copy` using the same action as HTTP; protocol 4 updates the worker contract and older clients must reload.
- Cover original-plan execution with the virtual-clock simulator, store restart, lost checkpoints, HTTP/MCP transport parity, stale requests and path rejection. Broader M3 durability/isolation gates remain open.

## 0.7.0 — 2026-10-26

- Start M3 with a shared update-drain admission guard: new Starts are refused while draining, accepted Start requests still replay, queued turns wait, and Stop remains independent.
- Report update waiting and readiness in the shared HTTP/MCP status. Readiness requires every cutter idle; paused, running and unconfirmed-stop executions block it.
- Add deterministic admission/update race tests and a simulator regression for Start replay, refused new Start, Stop, settling, and a queued turn surviving drain.
- This is updater infrastructure; deployment/restart wiring, immutable cut copies, durable cutter records and worker scheduling remain M3 work.

## 0.6.2 — 2026-10-26

- Finish M2 review fixes: establish the initial design snapshot and subscription under the publication gate, and coalesce slow SSE readers to the newest full snapshot.
- Serialize cross-session list reloads without holding the originating session lock; regression checks cover simultaneous catalog edits.
- Explain automatic draft recovery after worker failure, and report a planning failure as unavailable rather than a missing HTTP resource.

## 0.6.1 — 2026-10-26

- Fix M2 Open/Save retries to use the original complete request, before stale checks or source resolution; scope save keys to their person.
- Serialize session changes through Save, fence its completion, and compare library base revisions under the publication lock.
- Publish private save receipts with library revisions so a crash between publishing and session acknowledgement cannot duplicate a save; lookups recover the original result.
- Complete worker exchanges after HTTP cancellation, retain closed-session key records in draft schema 2 (schema 1 remains readable), and reject unknown draft schemas.
- Checkpoint and detach household work before the first admin adopts it; recheck access on design event streams.
- Protocol 3 requires reloading older clients.

## 0.6.0 — 2026-10-26

The material and mat lists are global (the developer, 2026-10-27; spec 1.1, §6, §8).

- **One list for the household,** in `data/catalog/`, shared by every person's session. Each
  person's other settings stay in their own area.
- **Edits never undo each other:** an edit takes a lock on the shared list and reads it again
  before changing it (`SettingsStore::with_shared_lists`, in the imported controller).
- **Everyone sees the change:** after an edit, every other loaded session reads the lists again,
  and its pages get the new snapshot.
- **Still to do:** the library's Adjust page should use this same list instead of cameo-pi's
  per-sender catalog; and the backup should cover `data/catalog/` (milestone 7).

## 0.5.0 — 2026-10-26

Milestone 2, step 5: opening and saving with other devices and people in mind (spec §7, §11).

- **Opening is a change of its own,** keyed and checked like any other. It opens one of the
  person's uploads by its owned id, or a library project, which is copied into their area first.
  Uploading now only stores the design.
- **Opening over unsaved changes** is refused with `unsaved-changes` until the person chooses to
  open anyway. The replaced project is then kept as a draft (the last ten are kept).
- **Save and Save As** put the project in the library through its own validated upload, as a new
  revision of the project it came from, or as a new project.
  - **Save names its base:** if someone saved that project since, it's refused with
    `saved-elsewhere`, so the person can Reload or Save As. Library revisions are append-only, so
    nothing is ever overwritten.
- **The snapshot says** whether there are unsaved changes, and which library revision the project
  is.
- **Worker messages** now live in `cutcraft_web_protocol::worker`, apart from the browser's
  contract.
- **`scripts/design-check.sh`** checks all of milestone 2 against the real binary, with workers as
  child processes, including a crash coming back from its draft. It replaces the milestone 0
  spike, now retired.

## 0.4.0 — 2026-10-26

Milestone 2, step 4: the authority policy and owned identifiers (spec §6).

- **One policy by the registry's own command classes** replaces milestone 0's fixed list:
  - **Allowed:** reads and edits of the person's own project.
  - **File commands:** refused from a session. Designs and projects go through the server's own
    routes, named by owned ids.
  - **Cutter commands:** refused from a session. Cutting goes through the cutter service
    (milestone 3).
  - **Refused by name,** though their class allows them: commands whose replies can carry server
    paths, the desktop app's own AI and station settings, and the cut screen.
- **A test runs every command the policy allows** against an open design and checks that no
  answer or refusal shows a server path. New registry commands are covered automatically.
- **Each person's uploads have an index:** `GET /api/v1/design/designs` lists their owned ids and
  names, never where they're stored, and `POST /api/v1/design/open` reopens one by id. Someone
  else's id is simply not found.

## 0.3.0 — 2026-10-26

Milestone 2, step 3: drafts and change records that survive a crash (spec §7).

- **Each person's open project is kept as a draft:** 5 seconds after the last change, at least
  every 30 seconds while changes keep coming, when a design is opened, and when the worker stops.
  Both timings are provisional.
- **A worker that stops comes back where it was:** the next request starts a fresh worker, under a
  new incarnation, from the draft.
- **Change records are saved with the draft that contains them,** in one file, so they can never
  disagree. After a recovery, looking up a key answers truthfully:
  - **applied:** the change is in the recovered project;
  - **refused:** with its reason;
  - **not applied:** lost with the changes after the last draft, so the device can send it again.
- **A damaged draft is set aside,** never deleted, and the session starts empty.
- **A worker whose connection closes** (the server stopping or letting go of the session) writes
  its draft before it leaves.

## 0.2.0 — 2026-10-26

Milestone 2, first steps: people and their design sessions (spec §6, §7).

- **Every account has a stable id,** for its whole life. Accounts from before get one the first
  time the file is read, saved once so it never changes.
- **Each request belongs to a person:** the signed-in account, or the household in open mode.
  When the first admin is created, what the household made becomes theirs.
- **Each person has a private area,** `data/people/<id>/` (designs, fonts, drafts), readable by no
  one else.
- **Design sessions on the server:** one per person, each with its own preparation worker
  running CutCraft's controller (`cameo-pi worker`, its memory capped). They sit behind the
  server's own login, CSRF and Host checks.
  - **The routes:** `/api/v1/design/…`: upload, command, lookup by key, snapshot, geometry,
    pictures, and live snapshots (server-sent events).
  - **Checks first:** every request is checked for the protocol version before it reaches a
    worker.
  - **Uploads** are stored in the person's own area under names the server chooses.
  - **A worker that stops** fails one request, and the next starts a fresh one.
- **Tested:** a household design, a refused protocol version, and two people with separate
  sessions and areas, where one can't fetch the other's artwork.

## 0.1.0 — 2026-10-26

Milestone 1: one self-contained workspace (spec §5.2).

- **CutCraft's and cameo-pi's code is here,** at recorded commits (`docs/provenance.md`):
  - **From CutCraft:** planning, the Cameo driver, the Simulated Cameo, opening and Make
    cut-ready, the command registry and the station client.
  - **From cameo-pi:** the library, the cutter workers, the server, backups and MCP.
  - **Their tests come with them:** 376 Rust tests, cameo-pi's backup checks and its browser checks.
- **No dependency on the old repositories:** a fresh public checkout builds with no access to
  either. `cargo deny` now checks dependency sources, licences and advisories in the Nix shell.
- **The end user is "they"** in all imported comments and in the command descriptions AI tools
  read.
- **Dependency updates from the merge:**
  - **tokio 1.54.1,** past an unsoundness advisory in 1.53.
  - **crypto-bigint 0.7.10:** 0.7.8 was yanked.
