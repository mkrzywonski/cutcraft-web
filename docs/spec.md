# CutCraft-Web specification

Status: first draft (2026-10-26), written before any code. It records the developer's decisions, the
design proposed for them, and the questions still open ([§18](#18-open-questions)). Items marked
**(proposed)** are still to be settled with the developer. Nothing here has been built or measured
yet; [§14](#14-performance-and-the-spike) lists what must be measured before hardware is bought.

## 1. Purpose

CutCraft-Web is [CutCraft](https://github.com/mkrzywonski/cutcraft) as a self-hosted web
service. One small home server with the Silhouette Cameo plugged in does everything CutCraft and
[cameo-pi](https://github.com/mkrzywonski/cameo-pi) do today. People use it from a browser on
whatever device is at hand:
- **On a computer or a large tablet:** CutCraft's own window, as it is today.
- **On a phone:** cameo-pi's thumb-friendly pages, grown to cover more.

**Why:** a desktop app has to be packaged, signed and supported on every platform. On macOS that
means a universal `.app` and Gatekeeper. On Windows it also means a USB driver for the Cameo. A
browser needs none of that, and the Cameo plugs into the server instead of a laptop.

**The tests for every feature:**
- **On a computer:** is it as good as the CutCraft desktop app?
- **On a phone:** could a non-technical user, standing at the cutter with only a phone, do this
  without calling for help?

## 2. Design goals

As set by the developer (2026-10-26):

1. **The journey ends with CutCraft-Web.** CutCraft and cameo-pi become one service. Both
   existing projects keep working: CutCraft on the desktop with a USB Cameo, cameo-pi on a
   Pi Zero 2 W.
2. **Architecture B:** CutCraft runs on the server, and the browser is the screen
   ([§5](#5-architecture)). A version that ran CutCraft inside the browser was considered and set
   aside. It would lose live MCP for the design being worked on, the automatic reload when the
   design file changes, and installed fonts in Safari.
3. **Two layouts in one app, by screen size** ([§7](#7-two-layouts)):
   - **Large screens get CutCraft's desktop arrangement.** The developer is very happy with
     CutCraft's UI.
   - **Small screens get the thumb-friendly arrangement.** The developer is very happy with
     cameo-pi's mobile UI.
   - **Why both:** the side-by-side view doesn't work on a phone (perhaps in landscape), and one
     layout won't be perfect on every device.
4. **Hardware:** a Raspberry Pi 5 with 8 GB and an SSD, or a ZimaBoard 2 (Intel N150, 8 GB). A
   Pi Zero 2 W can't run it. The choice waits for the measurements in
   [§14](#14-performance-and-the-spike).

## 3. Users, hardware and browsers

- **Users:** the end user, a hobbyist making vinyl projects, not technical, who designs in
  Illustrator on an Intel MacBook. And the developer. Each has their own login.
- **Server:**

| Board | CPU | Notes |
|---|---|---|
| Raspberry Pi 5, 8 GB, SSD | 4× Cortex-A76, aarch64 | Built-in Wi-Fi. ~3–10 W. |
| ZimaBoard 2, 8 GB | Intel N150, x86-64 | Roughly 30–45% faster single-threaded than the Pi 5 (published benchmarks). Ethernet only. Fanless; throttles under long multi-core load. ~7–21 W. |
| ZimaBoard 832 | Celeron N3450 | **Not suitable:** several times slower than a Pi 5. |

  Any always-on Debian machine (amd64 or arm64) with 8 GB also works. That's the route for
  Docker ([§15](#15-packaging-and-installing)).
- **Cutter:** an original Silhouette Cameo over USB, driven by cameo-pi's per-cutter worker.
  Several cutters work as they do in cameo-pi.
- **Browsers that must work:**

| Device | Browser | Layout |
|---|---|---|
| The end user's Intel MacBook | Safari, and Chrome | Desktop |
| Linux, Windows and Mac computers | Firefox, Chrome, Edge, Safari | Desktop |
| iPhone | Safari | Compact |
| Android phone | Chrome | Compact |
| iPad (landscape) | Safari | Desktop, touch and Pencil |
| iPad (portrait), small tablets | Safari, Chrome | Compact |

  The desktop layout needs WebGL2. Safari has had it since version 15; the end user's macOS
  version is to be confirmed, as it was for CutCraft.

## 4. Words the user sees

One app, one vocabulary. Users see CutCraft's words for preparing and cameo-pi's words for the
server and its cutters. The two clash on one word (open question 1):

| Word | Means |
|---|---|
| **Design** | The artwork as drawn (an SVG, PDF or Illustrator file). |
| **Project** (proposed) | A design with how it's cut: what to cut, material, mat, placement. A library item. cameo-pi calls this a **job**. |
| **Sheet** | The piece of material loaded and cut now. |
| **Library** | Every project, in folders, with notes and cut history. |
| **Cutter** | One Cameo plugged into the server, under its name. |
| **Material**, **Mat** | As in CutCraft. |
| **Make cut-ready**, **Test cut**, **Cut preview** | As in CutCraft. |
| **Load the mat** | As in cameo-pi: a fresh mat for each cut. |
| **User**, **Admin** | As in cameo-pi. |

Avoid "server", "session", "endpoint", "token", "WebAssembly" and "revision" in the interface.

## 5. Architecture

```text
Computer browser ── desktop layout (CutCraft's UI, WebAssembly) ──┐
Phone browser ───── compact layout (HTML, as cameo-pi's) ─────────┼── HTTPS + WebSocket ── one program
AI tool ─────────── MCP ──────────────────────────────────────────┘                        │
CutCraft desktop app ── station protocol (as with a cameo-pi) ───────────────────────────────┤
                                                                                            ├── design sessions (CutCraft's controller, one per user)
                                                                                            ├── cameo-pi: library, cutters, users, backups
                                                                                            ├── designs folder (network share) and fonts
                                                                                            └── USB ── Cameo(s)
```

- **One program** (`cutcraft-web`), started at boot, on one HTTPS address with one login. It is
  cameo-pi's server with design sessions added (proposed; open question 2).
  - **Inside it:** cameo-pi's cutters, library, users, pairing, backups and MCP, unchanged.
  - **What it adds:** a design session per user, each running CutCraft's controller
    (`cutcraft-control`, with the VectorCraft engine).
- **CutCraft's controller is the one authority for preparing.** Every feature is an *Action*, and
  every screen is drawn from a *Snapshot* ([§10](#10-between-browser-and-server)). This is already
  how CutCraft's window and MCP share everything. Here the network sits between the controller and
  the screen.
- **cameo-pi stays the one authority for cutting.** A session cuts through the station protocol to
  its own server over loopback, using the remote cutter that was hardened in CutCraft 1.4.1–1.4.2,
  so no safety rule is reimplemented. Calling the cutter worker directly is a later optimisation,
  only if measured.
- **Nothing is computed in the browser** except drawing and input. Previews and pictures are
  rendered on the server.

### 5.1 Repositories and crates

| Where | What changes |
|---|---|
| **CutCraft** | The desktop UI moves into a library crate (`cutcraft-ui`). It reaches the controller only through a `Backend` interface, and builds for `wasm32-unknown-unknown`. The desktop app becomes a thin shell with the local backend. Types the UI needs (Snapshot, Action and the views) move to a WebAssembly-safe crate if `cutcraft-control` itself can't build for it. |
| **cameo-pi** | The server becomes a library (`cameo-pi-server`) that another program can embed, with routes added from outside. The `cameo-pi` program for the Pi Zero stays. |
| **CutCraft-Web** (this repo) | The `cutcraft-web` program: sessions, the browser API, the remote `Backend`, the web shell for the desktop layout, the compact layout's new pages, packaging. |

- **Pinning:** CutCraft-Web pins both repositories to full commit hashes, as cameo-pi already pins
  CutCraft. VectorCraft stays at the revision CutCraft pins.
- **The desktop layout's model:** VectorCraft's own web build (`apps/vectorcraft-web`), an eframe
  web runner with font loading, file pick, download and drop. Its CI already builds the engine,
  text and egui crates for WebAssembly.

## 6. Sessions

- **One design session per user (proposed; open question 3).**
  - **Every device and every AI key** of that user sees and drives the same session. Start on the
    laptop and walk to the Cameo: the phone shows the same project, ready to cut. This is how the
    desktop app and an AI tool share one window today.
  - **Each user has their own session.** Two people prepare different projects at once.
- **Only what's on screen is per device:** the view, the zoom, the selection, which panel groups
  are folded, the chosen layout.
- **Changes from two devices** are ordered by the controller, as UI and MCP changes are today.
  Every device sees the result at once.
- **Sessions outlive the browser.** Closing the tab loses nothing.
  - **Restarts:** the open project is kept as an automatic draft in the user's area, and comes
    back after a restart, an update or a power cut.
  - **Explicit Save** writes it to the library ([§8](#8-files-the-library-and-fonts)).
- **Memory:**
  - **Idle sessions:** a session idle for a while (default 1 hour) is written to its draft and
    unloaded, then reloaded on the next visit.
  - **The limit:** the number of loaded sessions is capped by memory, measured in the spike. At
    the cap, the oldest idle session is unloaded first.
- **A cut in progress** belongs to the cutter, not the session, as in cameo-pi. Unloading a session
  never stops a cut.

## 7. Two layouts

### 7.1 Choosing the layout

- **One address.** The page chooses when it opens:
  - **Desktop:** at least 1024 CSS pixels wide, or at least 768 wide with a mouse or trackpad
    (`pointer: fine`).
  - **Compact:** everything else.
- **A switch in the menu** overrides the choice on that device, and the device remembers it:
  "Use the computer layout" or "Use the phone layout". Turning the device, or resizing the window
  across the threshold, offers the other layout once; it never switches by itself in the middle of
  work.
- **Switching** loses nothing, because the work is in the session ([§6](#6-sessions)).

### 7.2 Desktop layout

- **CutCraft's UI, compiled to WebAssembly. Not rewritten.** The same menu bar, toolbar, views
  (Side by side, Overlay, Cut preview), right panel and cut screen. Its behaviour follows
  CutCraft's spec, so features added to CutCraft's UI arrive in both.
- **What differs from the desktop app:**
  - **Opening a file** uses the browser's file picker, a drop onto the window, the designs folder
    ([§8](#8-files-the-library-and-fonts)) or the library. There is no system Open dialog.
  - **Save** writes to the library; **Download** saves a `.cutcraft` copy.
  - **Shortcuts the browser keeps** for itself (⌘W, ⌘Q, ⌘N) have menu items instead.
  - **The window title** is the tab's title.
  - **The cutter picker** lists the server's cutters, and, if wanted, paired cameo-pis elsewhere.
- **Responsiveness:**
  - **Drawing** is local: panning, zooming and hover draw from the last Snapshot without
    waiting.
  - **Dragging on the mat** moves locally, and the server confirms when it's dropped
    ([§14](#14-performance-and-the-spike)).
- **Touch:** the desktop layout on an iPad works with fingers and Apple Pencil, as VectorCraft's
  web build does.

### 7.3 Compact layout

cameo-pi's HTML pages, extended. Each screen does one thing, the controls are large, and a phone
in portrait is the design size.

**What it covers (the written list; anything else says "Open this on a computer"):**

| Area | On the phone |
|---|---|
| Library | Everything cameo-pi has: folders, search, sort, move, rename, Recently deleted, notes, history. |
| Cutters and cutting | Everything cameo-pi has: queue, Load the mat, Start, Pause, Stop, the live cut viewer, test cuts. |
| Adjust | cameo-pi's Adjust: size, place, material, mat. |
| **The open project** (new) | What the session has open on the computer, and the current sheet: "Ready to cut", or what it still needs, with **Cut** when it's ready. |
| **New project from a design** (new, proposed; open question 4) | Pick a file from the designs folder or the phone. Then one screen per step: Make cut-ready with its defaults, its questions as plain choice screens, what to cut (colours to tick), material, Cut preview with automatic placement, Cut. |
| **Materials and mats** (new) | View and edit the user's materials and mats. |
| Settings, users, backups | As in cameo-pi. |

**Computer only:**
- **On the mat:** dragging pieces, handles and typed positions. The phone places automatically
  and offers Adjust.
- **Comparing:** side by side and overlay, including Show me.
- **Detail:** inspecting the source, the panel arrangement and the full undo history (the phone
  has Undo for its own last step).
- **Tracing pictures** beyond the default threshold.

### 7.4 Rules that keep two layouts honest

1. **Every feature is an Action first.** That's already the rule because of MCP. A layout is a
   view, so most work is shared.
2. **Both layouts show the same truth.** Neither keeps its own copy of the project, a cutter or
   the queue.
3. **The compact list above is maintained.** A feature added to the desktop layout either joins
   the list or says "Open this on a computer"; it never half-works.
4. **Browser checks cover both layouts:** the compact layout at phone width (as cameo-pi's checks
   do today), the desktop layout at desktop size, and a switch between them mid-project.

## 8. Files, the library and fonts

- **The library is cameo-pi's library.** A library item is a project (a `.cutcraft`, which embeds
  the original design). It keeps its folders, notes, cut history and revisions.
  - **Opening it** loads it into the session.
  - **Save** adds a new revision.
  - **Save As** makes a new item in a chosen folder.
- **Opening a design:** a file picker, a drop onto the desktop layout, or the **designs folder**.
- **The designs folder (proposed; open question 5):** a network folder on the server, one per user,
  that Finder and Windows Explorer can connect to.
  - **Saving there from Illustrator** brings back CutCraft's automatic update: an open project
    whose design changes offers Update from Design, as on the desktop.
  - **Opening** that folder in the browser lists its files.
- **Fonts:**
  - **Bundled:** the server has VectorCraft's craft fonts.
  - **The user's own:** each user has a **fonts folder** (next to the designs folder) for fonts
    their designs use. Make cut-ready's font check runs against the bundled fonts and the user's.
  - **A missing font** says which one is missing and where to put it. CutCraft's design guide
    already recommends text as shapes, which needs no fonts.
- **Downloads:** any project as `.cutcraft`, and its cut lines as SVG, as CutCraft exports.
- **Uploads** are checked as cameo-pi checks them (its §10). Every design file opens on the
  server, never in the browser.

## 9. Cutting

- **The same cutting as cameo-pi:** turns, the queue, the fresh-mat rule, test cuts, live progress,
  Pause and Stop, and cut history. The cut screen in each layout shows the cutter's state.
- **From a session,** Cut prepares the sheet and goes through the remote cutter on loopback
  ([§5](#5-architecture)). Every guarantee that path has is kept: Start replay and lookup, Stop
  winning, and unanswered Starts kept under control.
- **The CutCraft desktop app** can still pair with CutCraft-Web as with any cameo-pi and cut
  there.
- **Physical safety rules are cameo-pi's** and stay in its worker.

## 10. Between browser and server

- **One protocol for the desktop layout,** versioned:
  - **Actions:** `POST` with the session's revision and the user's change. It is answered with the
    result, or with a refusal saying why.
  - **Snapshots:** pushed over a WebSocket at each new revision. Small parts go every time; the
    open design's geometry goes once per design revision, by its id.
  - **Pictures:** artwork renders, picture previews and thumbnails are fetched as images, by
    revision and region, and cached by the browser.
  - **Uploads and downloads:** plain HTTPS requests.
- **The compact layout** uses cameo-pi's JSON API and live events, with routes added for the open
  project, new projects and materials. Those routes run the same Actions.
- **Stale results:** a Snapshot or picture from an older revision never replaces a newer one, the
  same rule as CutCraft's workers.
- **Reconnecting:** after a dropped connection, the page reconnects and asks for the current
  Snapshot. It never replays a change it can't prove wasn't applied: changes carry keys, as
  cameo-pi's requests already do.
- **Security:** cameo-pi's rules apply unchanged (its §14): HTTPS with the server's own
  certificate, login sessions, CSRF and Host checks, a strict Content Security Policy (with
  `wasm-unsafe-eval` for the WebAssembly, nothing fetched from the internet), and passwords with
  argon2.

## 11. MCP

- **AI tools act as the person who made their key,** in that person's session, through the same
  Actions. This keeps CutCraft's rule that automation can do anything the user can.
- **What an AI tool can use:** CutCraft's design tools and cameo-pi's library and cutter tools.
  Whether that's one endpoint or two is open question 6.
- **Backup setup, users and keys stay web-only,** as in cameo-pi.

## 12. Users and security

- cameo-pi's users, admins, open mode (no users yet, with a banner), pairing, `reset-admin` and
  certificate handling, unchanged.
- **Per-user areas:** designs folder, fonts folder, drafts, materials and mats, and settings
  (units, font size, panel layout).
  - **The library is shared,** as in cameo-pi. Everyone sees every project.
  - **Who can open the folders:** each user only their own, signed in with their web password
    (open question 5).

## 13. Backups

- cameo-pi's library backup (schema 2) extends to the per-user areas. A restored server has every
  project, design, font, material and setting.
- Drafts are backed up too, so a failed SD card or SSD loses at most the last few minutes of
  unsaved work.

## 14. Performance and the spike

**Targets (proposed):**

| What | Target on the chosen board |
|---|---|
| First page load on the home network | ≤ 10 s; ≤ 2 s once cached |
| Opening a typical design and Make cut-ready | ≤ 3× the time on the developer's desktop, and ≤ 10 s |
| A change (tick a colour, change material) to the new preview | ≤ 300 ms |
| Dragging on the mat | drawn at the browser's frame rate; confirmed ≤ 150 ms after the drop |
| Memory | two loaded sessions with large designs, plus cameo-pi, within 4 GB |

**The spike, before any board is bought or any CutCraft code moves:**
1. **Speed:** time `cutcraft-cli prepare` and `plan` on the local design archive
   (`CUTCRAFT_CORPUS_DIR`) on the developer's desktop. Scale by published single-core ratios for
   the Pi 5 and the ZimaBoard 2, then confirm on the real board once one is chosen. The results
   stay in `local/`.
2. **Memory:** peak memory for one open session per archive design.
3. **WebAssembly:** whether `cutcraft-control` and the desktop UI's crates build for
   `wasm32-unknown-unknown`, and what doesn't (threads, `Instant`, the file dialog, USB).
4. **Arm:** an aarch64 build of `cutcraft-control` and `cutcraft-cli`, with the same results as on
   x86 for the archive.
5. **Safari:** a minimal eframe web page with CutCraft's fonts and a canvas the size of the main
   view, on the end user's macOS version.

## 15. Packaging and installing

- **The cameo-pi route:** a `.deb` with its signed apt repository and an install script. Both are
  built by a release workflow from a version tag. Here they're built for arm64 and amd64.
- **Raspberry Pi 5:** an SD-card or SSD image from Raspberry Pi OS Lite, as cameo-pi's.
- **ZimaBoard and other x86 machines:** Debian with the package, or Docker. ZimaOS is built around
  Docker, so a container image is a first-class route there, unlike on the Pi Zero.
  - **The container** needs the Cameo's USB device, the host network (for Bonjour) and ports 80 and
    443.
- **Updates:** `apt upgrade` (or a new container). An update restarts the service, which stops a
  cut: update between cuts.

## 16. Testing

- **The four cargo checks**, plus cameo-pi's browser and backup checks.
- **Protocol tests:** Actions, Snapshots, stale results, reconnects with repeated keys, two devices
  of one user changing the same project, two users at once.
- **Browser checks for both layouts** ([§7.4](#74-rules-that-keep-two-layouts-honest)), in headless
  Chromium. On the Mac mini, Safari is checked by hand.
- **Parity:** the desktop layout in the browser and the desktop app give the same Snapshots for the
  same Actions on the archive designs.
- **Acceptance with the end user:** their MacBook in Safari, and their phone, for a real project
  from design to cut. The developer is present for any motion.

## 17. Milestones

0. **The spike** ([§14](#14-performance-and-the-spike)) and the hardware choice.
1. **CutCraft: the UI split.** `cutcraft-ui` behind `Backend`. The desktop app works as before, and
   its checks pass.
2. **cameo-pi: the embeddable server.** `cameo-pi-server` as a library. The `cameo-pi` program for
   the Zero is unchanged.
3. **Sessions and the protocol:** the `cutcraft-web` program with sessions, Actions, Snapshots,
   pictures, drafts and logins.
4. **The desktop layout in the browser:** open, Make cut-ready, Cut preview, save to the library.
5. **Cutting from a session:** the cut screen in both layouts.
6. **The compact layout's additions:** the open project, new project from a design, materials and
   mats.
7. **The designs and fonts folders**, with Update from Design.
8. **MCP for sessions.**
9. **Packaging:** packages for arm64 and amd64, the Pi 5 image, and the container.
10. **Acceptance** with the end user.

## 18. Open questions

1. **One word for a library item:** CutCraft's "project" or cameo-pi's "job"?
   - **Proposed:** "project" everywhere, since a library item is a CutCraft project.
   - **The cost:** cameo-pi's pages change wording, and cameo-pi alone could keep "job".
2. **One program, or two on the same box?**
   - **Proposed:** one, with cameo-pi's server embedded as a library. That gives one address, one
     login, one install and one backup.
   - **The alternative:** CutCraft-Web as a second service behind cameo-pi. That isolates memory,
     but needs a shared login and a proxy.
3. **Sessions per person or per device?**
   - **Proposed:** per person ([§6](#6-sessions)).
4. **Should the phone start new projects from a design** ([§7.3](#73-compact-layout))? Or is it
   enough that the phone cuts and adjusts what a computer prepared, as with cameo-pi today?
   - **Proposed:** yes, with defaults and automatic placement.
5. **How does the designs folder appear on a computer?**
   - **SMB (Samba)** is the most reliable in Finder and Explorer, but needs its own passwords kept
     in step with the web logins.
   - **WebDAV** could be served by the program itself, with the same logins and certificate, but
     Windows' WebDAV client is unreliable.
   - **Proposed:** SMB, with each user's share password set from their web account page.
6. **One MCP endpoint or two?**
   - **One** (`/mcp`, both sets of tools) is simpler for the person setting up an AI tool, if the
     tool names are kept distinct.
   - **Two** keeps cameo-pi's MCP unchanged.
   - **Proposed:** one.
7. **The board:** Pi 5 or ZimaBoard 2, after the spike.
8. **What happens to the two older apps?**
   - **The CutCraft desktop app** stays for cutting with no server.
   - **cameo-pi** stays for the Pi Zero. Both keep being developed, or are frozen once
     CutCraft-Web replaces them for the end user. The developer decides once CutCraft-Web is
     accepted.

## 19. Out of scope

- Using CutCraft-Web from outside the home network. The developer's reverse proxy is the
  developer's own, as for cameo-pi.
- Several homes on one server, or user accounts beyond a household.
- Running the design work in the browser (Architecture A, [§2](#2-design-goals)).
- A native phone app.

## 20. Revision history

- **Draft 1 (2026-10-26):** first version, from the developer's decisions: Architecture B, two
  layouts by screen size, a Pi 5 or ZimaBoard 2.
