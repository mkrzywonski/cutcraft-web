# CutCraft-Web specification

Status: second draft (2026-10-26), written before any code. It records the developer's decisions
([§2](#2-decisions)), the design proposed for them, and the questions still open
([§23](#23-open-questions)). Items marked **(proposed)** are still to be settled with the developer.
Nothing here has been built or measured yet. Draft 1 was reviewed by Codex and Grok; draft 2 answers
those reviews ([§25](#25-revision-history)).

## 1. Purpose

CutCraft-Web is the one product: [CutCraft](https://github.com/mkrzywonski/cutcraft) and
[cameo-pi](https://github.com/mkrzywonski/cameo-pi) made into a self-hosted web service. One small home
server with the Silhouette Cameo plugged in does everything both did. People use it from a browser on
whatever device is at hand, with the same capabilities on each:
- **On a computer or a large tablet:** the desktop layout, following CutCraft's UI.
- **On a phone:** the compact layout, its own UI, based on cameo-pi's.

**Why:** a desktop app has to be packaged, signed and supported on every platform. On macOS that
means a universal `.app` and Gatekeeper. On Windows it also means a USB driver for the Cameo. A
browser needs none of that, and the Cameo plugs into the server instead of a laptop.

**The tests for every feature:**
- **On a computer:** is it as good as the CutCraft desktop app?
- **On a phone:** could a non-technical user, standing at the cutter with only a phone, do it
  without calling for help?

## 2. Decisions

From the developer, 2026-10-26:

1. **One product: CutCraft-Web.**
   - **The old products are retired.** The CutCraft desktop app and cameo-pi are not developed or
     shipped further.
   - **Their repositories are now private,** and will be deleted when CutCraft-Web is done.
   - **Their code** is brought into this repository ([§5.2](#52-one-self-contained-workspace)).
2. **Architecture B:** CutCraft's work runs on the server, and the browser is the screen. Running it
   inside the browser was set aside: it would lose live MCP for the open project and installed fonts
   in Safari.
3. **Two layouts, chosen by screen size, with the same capabilities**
   ([§9](#9-two-layouts-same-capabilities)).
   - **The desktop layout follows CutCraft.**
   - **The compact layout** is the phone's own UI, based on cameo-pi's.
   - **Same capabilities:** anything that can be done on a computer can be done on a phone.
4. **"Project"** is the word for a library item, everywhere.
5. **Settings per project and per person, as appropriate** ([§8](#8-settings)). For example, the
   theme is per person and units are per project.
6. **The phone can start new projects.**
7. **One MCP address.**
8. **No Samba, and no network share.** Designs and fonts arrive by upload in the browser
   ([§10](#10-designs-fonts-and-files)).
9. **One address and one login,** and **one design session per person**. Draft 1 proposed both; no
   reviewer or developer objected.
10. **Hardware:** a Raspberry Pi 5 with 8 GB and an SSD, or a ZimaBoard 2 (Intel N150, 8 GB). The
    choice follows measurements on the real boards ([§19](#19-performance-and-milestone-0)).

## 3. Users, hardware and browsers

- **Users:** the end user, a hobbyist making vinyl projects, not technical, who designs in
  Illustrator on an Intel MacBook. And the developer. Each has their own login.
- **Server:**

| Board | CPU | Notes |
|---|---|---|
| Raspberry Pi 5, 8 GB, SSD | 4× Cortex-A76, aarch64 | Built-in Wi-Fi. ~3–10 W. |
| ZimaBoard 2, 8 GB | Intel N150, x86-64 | Roughly 30–45% faster single-threaded than the Pi 5 (published benchmarks, to be measured). Ethernet only. Fanless; throttles under long multi-core load. ~7–21 W. |
| ZimaBoard 832 | Celeron N3450 | **Not suitable:** several times slower than a Pi 5. |

  Any always-on Debian machine (amd64 or arm64) with 8 GB also works. That's the route for Docker.
- **Cutter:** an original Silhouette Cameo over USB, driven by the cutter service
  ([§14](#14-cutting)). Several cutters work, as in cameo-pi.
- **Browsers that must work:**

| Device | Browser | Layout |
|---|---|---|
| The end user's Intel MacBook | Safari, and Chrome | Desktop |
| Linux, Windows and Mac computers | Firefox, Chrome, Edge, Safari | Desktop |
| iPhone | Safari | Compact |
| Android phone | Chrome | Compact |
| iPad (landscape) | Safari | Desktop, touch and Pencil |
| iPad (portrait), small tablets | Safari, Chrome | Compact |

- **What the desktop layout needs:** WebGPU, falling back to WebGL2 where it's missing, as
  VectorCraft's web build does. WebGPU needs a secure context, which the server's HTTPS provides.
  WebGL2 is in Safari 15 and later; the end user's macOS version is to be confirmed.

## 4. Words the user sees

| Word | Means |
|---|---|
| **Design** | The artwork as drawn: an SVG, PDF or Illustrator file, uploaded. |
| **Project** | A design with how it's cut: decisions, what to cut, material, mat, placement. A library item. |
| **Sheet** | The piece of material loaded and cut now. |
| **Library** | Every project, in folders, with notes and cut history. |
| **Cutter** | One Cameo plugged into the server, under its name. |
| **Material**, **Mat** | As in CutCraft. |
| **Make cut-ready**, **Test cut**, **Cut preview** | As in CutCraft. |
| **Load the mat** | As in cameo-pi: a fresh mat for each cut. |
| **User**, **Admin** | As in cameo-pi. |

Avoid "job", "server", "session", "endpoint", "token", "WebAssembly", "worker" and "revision" in the
interface. Only the label changes: the library's storage format can stay as it is
([§16](#16-the-library)).

## 5. Architecture

```text
Computer browser ── desktop layout (wasm) ──┐
Phone browser ───── compact layout (HTML) ──┼── HTTPS + WebSocket ── cutcraft-web (one address, one login)
AI tool ─────────── MCP ────────────────────┘                          │
                                                                       ├── front: logins, authority, browser API, MCP, static files
                                                                       ├── library, users, backups
                                                                       ├── cutter service ── USB ── Cameo(s)
                                                                       └── preparation workers, one per loaded session
                                                                           (CutCraft's controller with the VectorCraft engine)
```

- **One installed program, one address, one login.**
- **The front** owns HTTPS, logins, the authority policy ([§6](#6-who-may-do-what)), the browser
  API ([§13](#13-between-browser-and-server)), MCP, the library, users and backups.
- **The cutter service** owns the cutters, as cameo-pi's per-cutter workers do: turns, the queue, the
  fresh-mat rule, Start, Pause and Stop. It is small and never parses a design.
- **Each person's design session runs in a preparation worker** (proposed): a child process running
  CutCraft's controller. It does the opening, Make cut-ready, planning and rendering. That isolates
  the risky part from the cutter service ([§18](#18-resources-and-isolation)):
  - **Malformed files and very large designs** stay inside that one worker.
  - **Memory per worker is limited.**
  - **A crash** loses at most that session's last unsaved step, never a cut.

  If milestone 0 shows that threads in one process meet the same isolation tests, the workers can be
  threads instead. Either way, the tests in §18 decide.
- **CutCraft's command registry is the one validated path for preparing.** It is about 80
  commands (`design.open`, `prepare.decide`, `sheet.place`, `view.show`…), each with a params
  schema, a read or edit class, and availability (`crates/control/src/commands.rs`). The window, MCP
  and both layouts use the same registry. No second action enum is made.
- **The cutter service is the one authority for cutting.** Cuts arrive as frozen copies of a
  project ([§14](#14-cutting)).

### 5.1 Crates

| Crate | Origin | Role |
|---|---|---|
| `cutcraft-job`, `cutcraft-path`, `cutcraft-device`, `cutcraft-sim` | CutCraft | Projects, planning, the Cameo driver, the Simulated Cameo. |
| `cutcraft-import`, `cutcraft-control` | CutCraft | Opening, Make cut-ready, the command registry. They run in the preparation workers only. |
| `cutcraft-station` | CutCraft | The cutter protocol between workers and the cutter service. |
| `cameo-pi-core` | cameo-pi | The library, the cutter workers, validation, the library backup. |
| `cutcraft-web-protocol` (new) | — | The browser-safe contract ([§13](#13-between-browser-and-server)). Builds for wasm. |
| `cutcraft-ui` (new) | CutCraft's desktop app | The desktop layout's egui code, behind the protocol. Builds for wasm. |
| `cutcraft-web` (new) | cameo-pi's server and new code | The program: front, cutter service, workers, MCP, the compact layout's HTML. |

VectorCraft stays an external dependency, pinned to a full commit, as CutCraft pins it now.

### 5.2 One self-contained workspace

- **Import the code, not the history** (proposed). The crates above come in at recorded origin
  commits, with a `docs/provenance.md` listing each one's origin repository, commit and licence (both
  are MIT or Apache-2.0).
  - **What's left out:** history, local artefacts and anything private (the privacy rules in
    `AGENTS.md`).
- **What else comes in:** the tests that matter, including the protocol, cutter, remote Start/Stop
  and backup regressions, and the browser checks.
- **cameo-pi's 0.23.0 packaging work** (the `.deb`, the signed apt repository, the install script,
  the SD image) moves here too.
- **Acceptance:** a fresh public checkout builds and passes the four cargo checks with no
  credentials for either old repository, including every `Cargo.lock` source.
- **The CutCraft desktop app is kept only as a test harness:** the same UI over a local backend, for
  parity tests ([§22](#22-testing)). It is not shipped.

## 6. Who may do what

The old desktop commands assume one person's whole filesystem. On a household server they must not
let one person, or their AI key, reach another person's work or the server's secrets.

- **The principal:** every request is identified once, as the signed-in person, an AI key, or the
  open-mode household. The browser session, its WebSocket and that person's MCP key are bound to
  that person.
- **The authority policy** is enforced on the server, for every command and route:
  - **Each command has a class:** read, edit, cut or admin. Admin commands are web-only, as backup
    setup, users and keys are in cameo-pi.
  - **Owned identifiers, never paths.** Browser and MCP requests name files by identifiers the
    server issued: designs, fonts, drafts, library projects, exports. The adapter resolves an
    identifier only inside the person's own area, or the shared library, refusing traversal and
    symlinks. Linked resources inside a design (images, fonts referenced by path) are refused or
    resolved only within the upload.
  - **The same checks** cover images, geometry, downloads and WebSocket subscriptions.
- **Areas:**
  - **Each person's own:** designs, fonts, drafts, settings and their material list. No one else
    can read it.
  - **The library is shared:** every user can open, save, move and delete any project, as in
    cameo-pi's household model.
- **Open mode** (no users yet, as in cameo-pi): one household principal. When the first admin is
  created, the household's area becomes theirs.
- **Cutting keeps the actor.** A worker reaches the cutter service as its person, with a per-person
  internal credential, so the queue, history and audit name who cut. There is no shared
  administrator key.

## 7. Sessions

- **One design session per person.**
  - **What it holds:** the project open now, its Undo history, and the desktop screen
    ([§8](#8-settings)).
  - **Who drives it:** every device and AI key of that person, so work moves from the laptop to the
    phone with nothing lost.
  - **Each person has their own,** so two people can prepare different projects at once.
- **Every change** is a registry command carrying:
  - **an idempotency key,**
  - **the session's incarnation:** a fresh id each time a worker loads the session, so a command
    meant for an earlier load can't match the same revision in a fresh one,
  - **the revision it was made against.**

  The worker processes it atomically:
  - **A key already accepted** returns its recorded result, before any stale-revision check.
  - **The same key with a different body** is refused.
  - **An unsure device** asks for the key's result after reconnecting, instead of sending again.
- **Opening another project** replaces the session's project for every device. If the current one has
  unsaved changes, the person is told on that device first, and the previous project stays as a
  draft that can be reopened. Other devices see the new project, with a note saying where it was
  opened.
- **Undo** is the latest change to the shared project, whoever made it, labelled with where it came
  from ("on the phone", "by the AI tool"). There's no selective undo.
- **Drafts:** the open project is written to its draft 5 seconds after the last change (proposed),
  when the session unloads, and when the program stops. The draft write is the durability point:
  after a crash or power cut, the session comes back as of its last draft.
- **Unloading:** a session idle for an hour (proposed) is written to its draft and unloaded, then
  reloaded on the next visit. At the memory cap ([§18](#18-resources-and-isolation)), the
  longest-idle session unloads first. If none can unload, opening a design waits and says so.

## 8. Settings

Each setting lives where it belongs:

| Where | Settings | Stored |
|---|---|---|
| **Per project** | Units, snap and its step, the decisions, what to cut, material, mat, placement, the test-cut spot. | In the project (`.cutcraft`). A saved project keeps its material, so editing a material list never changes saved projects. |
| **Per person** | Theme, interface font size, the desktop panel arrangement, the units, material and mat a new project starts with, their material and mat lists, their fonts. | In their area, backed up. |
| **Per device** | Zoom and pan, which panel groups are folded, the layout override (computer or phone). | In that browser only. |
| **Per session** | The desktop layout's screen (Side by side, Overlay, Cut preview, cut screen), set by `view.show`. It is shared by the person's desktop layouts and MCP, as one window is today. | In the session. |

- **The compact layout never calls `view.show`.** Its screens are pages, so opening the cut page on
  the phone leaves the laptop on Cut preview.
- **What changes from CutCraft:** units and snap move from the app settings into the project; a new
  project takes the person's default. The controller change happens at import ([§5.2](#52-one-self-contained-workspace)).

## 9. Two layouts, same capabilities

### 9.1 Choosing the layout

- **One address.** The page chooses when it opens:
  - **Desktop:** at least 1024 CSS pixels wide, or at least 768 wide with a mouse or trackpad
    (`pointer: fine`).
  - **Compact:** everything else.
- **A switch in the menu** overrides the choice on that device, and the device remembers it.
  Turning the device or resizing across the threshold offers the other layout once; it never
  switches by itself in the middle of work.
- **Switching loses nothing:** the work is in the session.

### 9.2 Desktop layout: follows CutCraft

- **CutCraft's UI, compiled to WebAssembly, not rewritten:** its menu bar, toolbar, views (Side by
  side, Overlay, Cut preview), right panel and cut screen. It follows CutCraft's spec, which comes
  into this repository as `docs/cutcraft-ui.md`.
- **What differs from the desktop app:**
  - **Open:** the browser's file picker, a drop onto the window, the person's uploaded designs, or
    the library.
  - **Save** writes to the library ([§16](#16-the-library)); **Download** saves a `.cutcraft` copy.
  - **Update from Design:** upload the newer file ([§10](#10-designs-fonts-and-files)).
  - **Browser shortcuts:** ones the browser keeps for itself (⌘W, ⌘Q, ⌘N) have menu items instead.
  - **The cutter picker** lists the server's cutters.
- **Drawing** is local ([§12](#12-drawing-in-the-browser)).
- **Touch:** the desktop layout on an iPad works with fingers and Apple Pencil, as VectorCraft's web
  build does.

### 9.3 Compact layout: the phone's own UI

cameo-pi's HTML pages, extended to the full workflow. Each screen does one thing, the controls are
large, and a phone in portrait is the design size. **The same capabilities as the desktop layout,
presented for a phone:**

| Capability | On the phone |
|---|---|
| Library | cameo-pi's pages: folders, search, sort, move, rename, Recently deleted, notes, history. |
| Open and new projects | Upload a design (from the phone's files or photos), or open an uploaded design or a library project. |
| Make cut-ready | Its report, and its questions as choice screens with *Show me*, which zooms to the objects concerned. |
| Comparing | One view at a time, swiped or toggled: artwork, cut lines, overlay. This replaces side by side, which doesn't fit a phone. |
| What to cut | The colours or layers as a ticked list. |
| Material and mat | Choose, adjust (blade, force, speed, passes, mirror, overcut), save; the material and mat lists. |
| Cut preview and placement | The mat, with touch dragging of the design and the test-cut spot, two-finger resize, typed positions and sizes, Fit, snap. |
| Cutting | cameo-pi's pages: cutter choice, test cut, queue, Load the mat, Start, Pause, Stop, the live cut viewer. |
| Undo and Redo | The shared history ([§7](#7-sessions)). |
| Save, Save As, Download, Update from Design | As on the desktop. |
| Settings | The person's settings, fonts, users, backups (admin), AI keys. |

- **Same capabilities is a rule, checked by a test.** Every user-facing registry command must be
  reachable from both layouts, or be listed in `docs/layout-exceptions.md` with the developer's
  agreement ([§22](#22-testing)). That list starts empty.
- **Presentation may differ, the outcome may not.** The phone may reach a result in more steps, or
  with a different control, but it reaches the same result through the same commands.

### 9.4 Rules that keep two layouts honest

1. **Every feature is a registry command first.** That's already the rule because of MCP. A layout
   is only a view.
2. **Both layouts show the same truth.** Neither keeps its own copy of the project, a cutter or the
   queue.
3. **The parity test** ([§9.3](#93-compact-layout-the-phones-own-ui)) and the browser checks run for
   both layouts.

## 10. Designs, fonts and files

- **Designs arrive by upload:** the file picker or a drop. Each upload becomes a server-owned copy
  in the person's area, listed under their designs.
- **Update from Design:** uploading a newer version of a project's design keeps the project's
  decisions where they still apply, as CutCraft's Update from Design does today. There is no
  automatic watch of the person's own computer.
- **Fonts:**
  - **Bundled:** the server has VectorCraft's craft fonts.
  - **The person's own:** each person can upload fonts in Settings, for designs that use them.
  - **Make cut-ready's font check** runs against the bundled fonts and that person's own, in a
    font context of their own, never shared between people. A worker per session provides this
    naturally ([§5](#5-architecture)).
  - **A missing font** says which one, with "Upload a font" or the design guide's advice to save
    text as shapes.
- **Untrusted input:** every upload is checked before use: size, type, decoded picture size, page
  count ([§18](#18-resources-and-isolation)). It is opened only in a preparation worker.
- **Downloads:** a project as `.cutcraft`, and its cut lines as SVG.

## 11. Saving

- **Save names its base:** the library revision the project was opened from, as well as the
  session's revision. If someone else saved that project since, Save offers **Reload** (their
  version) or **Save As** (a new project), and never overwrites silently.
- **Save As** makes a new library project in a chosen folder.
- **Saving keeps everything** the project needs to be opened again by anyone: its design, decisions,
  material, mat and placement ([§8](#8-settings)).

## 12. Drawing in the browser

The desktop draws cut lines as vectors every frame, from the in-memory project, and the artwork as a
picture of the visible region at the current zoom. The browser does the same:

- **The session snapshot is small:** readiness, the issue and info cards, the rows, the sheet's
  settings, the cutter's state, errors. It carries no geometry, no pixels and no server paths.
- **Geometry:** the browser keeps a copy of the prepared vectors (cut lines, the mat, the
  test-cut spot). It is fetched by an id that changes whenever they change, and drawn locally every
  frame.
- **Pictures:** the artwork for a region is fetched by everything that affects it: the design
  revision, the prepared revision, the region and the zoom step. While a new one is on its way, the
  browser keeps showing the previous picture, scaled. That fetch has its own target
  ([§19](#19-performance-and-milestone-0)).
- **Dragging** is local until it's dropped, then one command with millimetres and the revision it
  was made against: `sheet.place`, `sheet.piece` or `sheet.test_spot`. If another device or the AI
  tool changed the placement first, the last accepted command wins and the dropped piece settles
  where the new snapshot puts it. The desktop app's settling rule stays: a drop is held on screen
  until the new plan arrives, or two seconds pass.

## 13. Between browser and server

- **`cutcraft-web-protocol`** is the one browser-safe contract, shared by the server and the desktop
  layout's wasm. It contains:
  - **command requests** (id, params, key, incarnation, revision),
  - **results and refusals,**
  - **command availability,**
  - **the session snapshot,**
  - **geometry and picture references,**
  - **upload and download references.**

  It holds no controller types, paths, settings files or service details. The controller's
  registry stays the authority; the protocol carries its ids and params. Its version is checked on
  connecting.
- **Transport:**
  - **Commands:** `POST` requests.
  - **Snapshots:** pushed over a WebSocket at each new revision.
  - **Geometry, pictures, uploads and downloads:** plain HTTPS requests.
- **The compact layout** uses the same commands through the same front. Its page routes are
  cameo-pi's, extended.
- **Stale results:** a snapshot, geometry or picture from an older revision never replaces a newer
  one.
- **Reconnecting:** the page reconnects, asks for the current snapshot, and looks up any command it's
  unsure of by its key ([§7](#7-sessions)). It never sends a change again blindly, and it never
  starts a cut by itself ([§14](#14-cutting)).
- **Serving the web shell,** as VectorCraft's web build is served:
  - **The wasm** goes as `application/wasm`, compressed with Brotli or gzip.
  - **Caching:** hashed wasm and JavaScript files are cached as immutable.
  - **WebGPU,** with the WebGL2 fallback.
  - **No `SharedArrayBuffer`,** so no COOP/COEP headers are needed.
  - **The size** is about 13 MB, about 5 MB compressed, for VectorCraft's own shell; CutCraft's is to
    be measured.
- **Security:** cameo-pi's rules apply.
  - **HTTPS** with the server's own certificate.
  - **Login sessions,** with CSRF and Host checks.
  - **Passwords** with argon2.
  - **A strict Content Security Policy,** adding only `wasm-unsafe-eval` for the WebAssembly, with
    nothing fetched from the internet.

## 14. Cutting

- **The same cutting as cameo-pi:** turns, the queue, the fresh-mat rule, test cuts, live progress,
  Pause and Stop, and cut history, in both layouts.
- **A frozen copy:** Cut makes a validated, immutable copy of the project's current sheet and its
  plan. The queue entry, the loading screen and Start all name that copy, so later edits in the
  session can't change what's cut. Editing after Cut makes a new copy only if Cut is pressed again.
- **One authority:** for the actor, the turn, the mat generation, the action epoch and request keys,
  across both layouts and MCP.
- **Start and Stop survive everything:** a session unloading, a worker crashing, a reconnecting
  phone, a deleted history entry, a restart.
  - **The cutter service's own records** keep accepted requests, outstanding Stop intent and how
    each cut ended, separately from sessions and from the ordinary, deletable cut history.
  - **A reconnecting device** resumes watching; it never sends a new Start.
  - **CutCraft 1.4.1–1.4.2's remote-cutter rules carry over, and are reviewed again for the new
    structure, not assumed:**
    - a Start is always answered with its first result;
    - Start can be looked up by key;
    - Stop wins over a Start in flight;
    - an unanswered Stop is kept and resent;
    - only one Start at a time.
- **Updates and cuts:**
  - **An update never restarts the program during a cut.** The package's install step asks the
    running program; if a cutter is busy, the restart waits until every cutter is idle, and the
    page says an update is waiting.
  - **A forced stop or a power cut** is recorded as interrupted, and motion never resumes by
    itself.
- **Physical safety rules are cameo-pi's,** and stay in the cutter service.

## 15. MCP

- **One address:** `https://<the server>/mcp`, with distinct tool names for the design commands and
  the library and cutter tools.
- **Each key acts as the person who made it,** in that person's session, through the same registry
  and authority policy ([§6](#6-who-may-do-what)). Keys name objects by explicit ids and revisions,
  never by a device's view.
- **Admin stays web-only:** backup setup, users and keys.

## 16. The library

- **cameo-pi's library,** shared by the household: folders, notes, cut history, Recently deleted,
  search and sort.
- **A project** is a `.cutcraft` (which embeds its design), stored as cameo-pi stores a job: a
  folder with its details and numbered revisions. "Project" is only the label, so the storage
  stays.
- **Opening** a project loads it into the session. **Save** adds a revision ([§11](#11-saving)).

## 17. Users and security

- cameo-pi's users, admins, open mode, `reset-admin` and certificate handling.
- The authority policy ([§6](#6-who-may-do-what)) applies to every route, every command and MCP.
- **Removing a user:** that person's own area goes to Recently deleted; their library projects
  stay.

## 18. Resources and isolation

Preparation parses untrusted files and can use a lot of memory. Cutting must not notice.

- **Limits** (proposed; measured in milestone 0):
  - **Uploads:** 64 MB per file.
  - **Pictures:** 100 megapixels decoded per design.
  - **PDFs:** 100 pages.
  - **Memory:** 1.5 GB per preparation worker.
  - **Concurrency:** at most two opens or Make cut-readies at once on the server. Others wait in
    line, and the page says so.
- **Cancellation:** opening, preparing and rendering can be cancelled, and are cancelled when their
  result is no longer wanted.
- **Backpressure:** a slow client never slows other clients or the cutter service. Its snapshots
  are coalesced to the latest, and its picture requests are dropped when superseded.
- **The cutter service is protected:**
  - **It runs apart from the workers** ([§5](#5-architecture)).
  - **Its memory is reserved,** and it is the last thing the system would stop.
  - **Stop is answered within 200 ms (proposed) under any preparation load.**
- **Acceptance, on the chosen board:**
  - **The load:** two sessions preparing large designs, a malformed file, a worker over its memory
    limit and a slow client.
  - **Meanwhile:** simulated cuts are started and stopped, and Stop's response time and the memory
    use are recorded.

## 19. Performance and milestone 0

**Targets (proposed), confirmed on the real board before it's chosen:**

| What | Target |
|---|---|
| First page load on the home network | ≤ 10 s; ≤ 2 s once cached |
| Opening a typical design and Make cut-ready | ≤ 3× the developer's desktop, and ≤ 10 s |
| A change (tick a colour, change material) to the new preview | ≤ 300 ms |
| A new artwork picture after pan or zoom stops | ≤ 300 ms (the old one shows meanwhile) |
| Dragging on the mat | the browser's frame rate; confirmed ≤ 150 ms after the drop |
| Stop under load | ≤ 200 ms |
| Memory | two loaded sessions with large designs, plus the cutter service, within 4 GB |

**Milestone 0 is a thin working slice, not a compile check.** On the developer's desktop, before the
UI split:
1. **The contract:** `cutcraft-web-protocol` with real command requests, the snapshot, geometry and
   picture references.
2. **The slice:**
   - sign in;
   - upload an authored fixture design, opened in a worker;
   - see the real Cut preview in the browser (wasm), with geometry drawn locally and a server
     picture;
   - change the material;
   - drag the design and drop it;
   - drop the connection mid-command and recover by key.
3. **Where it runs:** in Safari, Chrome and Firefox, with touch on an iPad, under the real CSP and
   serving headers. The wasm and transfer sizes are recorded.
4. **Speed on the developer's archive:** `cutcraft-cli prepare` and `plan` timed on the design
   archive (`CUTCRAFT_CORPUS_DIR`), with memory per design. Results stay in `local/`. Benchmark
   ratios give only an estimate; the board is chosen after timing the same runs on a Pi 5 and a
   ZimaBoard 2.
5. **Isolation:** worker processes against threads, by the §18 tests.

## 20. Backups and migration

- **cameo-pi's backup, extended** (schema 3, proposed):
  - **Per person:** each person's area, under their user id, with their name recorded.
  - **Unchanged:** the library, and schema 2's immutable-generation rules and retained-key
    warnings.
  - **Never backed up:** passwords and login sessions, AI keys, the backup's SSH key, the TLS key,
    pairings.
  - **Restoring older backups:** schema 1 and 2 restore as libraries only.
- **Restore maps people:** areas are matched to users by name. An area with no matching user is
  offered to an admin, who can give it to a user or leave it in Recently deleted.
- **Two kinds of recovery, and the interface keeps them apart:**
  - **On the same machine,** after a crash or power cut: drafts ([§7](#7-sessions)).
  - **Off the machine:** the last successful backup. The page shows when that was, and whether a
    backup is pending or failing. It never promises a fixed recovery point.
- **One writer per backup folder,** as in cameo-pi.
- **Migration from today's setup:** a cameo-pi library and backup, and CutCraft desktop projects
  (`.cutcraft`), import into CutCraft-Web. Tested with authored fixtures: a restored or migrated
  server reopens, edits, prepares and cuts a project on the Simulated Cameo, not only lists it.

## 21. Packaging and installing

- **cameo-pi 0.23.0's route, moved here:**
  - **The packages:** a `.deb` for arm64 and amd64, with its signed apt repository on this public
    repository's GitHub Pages.
  - **The install script.**
  - **The release workflow,** started by a version tag.
- **Raspberry Pi 5:** an SD-card or SSD image from Raspberry Pi OS Lite, as cameo-pi's.
- **ZimaBoard and other x86 machines:** Debian with the package, or a container image.
  - **The container** needs the Cameo's USB device, the host network (for Bonjour) and ports 80 and
    443.
- **Updates:** `apt upgrade` or a new container, never restarting during a cut
  ([§14](#14-cutting)).

## 22. Testing

- **The four cargo checks;** the imported cutter, protocol, remote Start/Stop and backup
  regressions; the browser and backup checks.
- **Protocol tests:**
  - commands with repeated and changed keys;
  - stale revisions and incarnations;
  - reconnects;
  - two devices of one person changing one project;
  - two people at once;
  - Save conflicts on a shared project.
- **The layout parity test:** every user-facing registry command is reached by a browser check in
  both layouts, or listed in `docs/layout-exceptions.md` (empty at the start).
- **The desktop parity test:** the desktop layout and the test-harness desktop app give the same
  snapshots for the same commands on authored fixtures, and on the developer's archive locally.
- **Cutting on the Simulated Cameo, early:**
  - two lost Start replies after the cutter accepted;
  - a late accepted Start after a failed Stop;
  - a session unloading or a worker crashing during reconciliation;
  - a deleted cut history entry;
  - a restart;
  - an update while busy.
- **Isolation and migration:** the acceptance runs in §18 and §20.
- **Acceptance with the end user:** their MacBook in Safari and their phone, for a real project from
  design to cut, each on its own. The developer is present for any motion.

## 23. Open questions

1. **Import with or without history** ([§5.2](#52-one-self-contained-workspace))?
   - **Proposed:** without, with a provenance file. That carries nothing private along by
     accident, and the old repositories will be deleted anyway.
2. **Workers: processes or threads?**
   - **Proposed:** processes. Milestone 0 decides by the isolation tests ([§18](#18-resources-and-isolation)).
3. **The board:** Pi 5 or ZimaBoard 2, after timing both ([§19](#19-performance-and-milestone-0)).
4. **The limits and timings marked proposed** (§7, §18, §19): the developer can change any of them;
   milestone 0 measures them.

## 24. Milestones

In the order the reviews recommended: contracts and risks first, layouts after.

0. **The contract and the thin slice** ([§19](#19-performance-and-milestone-0)).
1. **The self-contained workspace:** the crates and their tests imported; the four checks pass on a
   fresh public checkout.
2. **Authority and sessions:** principals, owned identifiers, drafts, idempotency, incarnations,
   Save conflicts.
3. **The cut handoff and isolation:** frozen copies, the cutter service apart from the workers, the
   Start/Stop simulator regressions, Stop under load, updates that wait.
4. **The desktop layout,** complete: everything CutCraft's UI does.
5. **The compact layout,** complete: the same capabilities, the parity test passing.
6. **MCP** at one address, with session parity.
7. **Backups and migration** (schema 3, people mapping, importing today's library and projects).
8. **The board measured and chosen; packaging** (packages, the Pi 5 image, the container).
9. **Acceptance** with the end user, the developer present for motion.

## 25. Out of scope

- Using CutCraft-Web from outside the home network. The developer's reverse proxy is the
  developer's own.
- Several households on one server.
- Running the design work in the browser (Architecture A).
- A native phone app.
- Samba or any network share (the developer, 2026-10-26).
- A Cameo plugged into the person's own computer. The Cameo plugs into the server.
- Shipping the CutCraft desktop app or cameo-pi again.

## 26. Revision history

- **Draft 2 (2026-10-26):** after reviews by Codex and Grok, and the developer's decisions.
  - **One product, self-contained:** the code is imported, the old products retired (§2, §5.2).
  - **The command registry** is the one path, not the Action enum. A browser-safe protocol crate
    (§5, §13).
  - **Authority:** principals, owned identifiers, per-person areas and fonts (§6).
  - **Sessions:** idempotency with incarnations, opening another project with unsaved work,
    shared Undo, draft timing (§7).
  - **Settings** per project, person, device and session. Units move into the project (§8).
  - **The same capabilities on both layouts,** with a parity test. The phone keeps its own UI (§9).
  - **No Samba:** designs and fonts by upload, Update from Design by upload (§10).
  - **Save conflicts** (§11).
  - **Drawing:** a small snapshot, local geometry, and pictures keyed by region and zoom step (§12).
  - **The web shell's serving requirements** (§13).
  - **Cutting:** a frozen copy, Start/Stop records outside sessions, updates that wait for idle
    cutters (§14).
  - **Resource limits and isolation**, with Stop under load (§18).
  - **Milestone 0 is a working slice,** measured on real boards (§19).
  - **Backups (schema 3) and migration** (§20).
- **Draft 1 (2026-10-26):** first version: Architecture B, two layouts by screen size, a Pi 5 or
  ZimaBoard 2.
