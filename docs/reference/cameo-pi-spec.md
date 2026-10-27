# cameo-pi specification

Status: fifth draft (2026-10-24), written before any code and revised after two rounds of design review
([§21](#21-revision-history)). It records the agreed design goals, the design proposed for them, and
the questions still open ([§18](#18-open-questions)). Nothing here has been tested on hardware yet.
Items marked **(proposed)** are a reviewer's proposal still to be settled with the developer.

## 1. Purpose

cameo-pi turns a Raspberry Pi Zero 2 W plugged into one or more Silhouette Cameo cutters into a
**network cut server**, in the spirit of OctoPrint for 3D printers. The cutters can live in
another room. People prepare designs in [CutCraft](https://github.com/mkrzywonski/cutcraft) on
their own computers and send them to the Pi, which keeps every job in a **library** and cuts it on
request, now or later, from CutCraft, from a phone, or from an AI tool.

**The test for every feature:** could a non-technical user, standing at the cutter with only a
phone, do this without calling for help?

## 2. Design goals

The goals as set by the developer, with the decisions taken since:

1. **Another cutter in CutCraft.** Each cutter on the Pi appears in CutCraft's cutter list. Everything else in
   CutCraft stays the same as cutting on a USB-connected Cameo.
2. **Progress on CutCraft's Cut screen**, as for a USB Cameo. It matters more when the cutter is in
   another room and can't be seen.
3. **A web interface** for all setup, and for managing jobs.
4. **A cut viewer on the web**, like OctoPrint's G-code viewer: the same picture of the job and its
   progress as CutCraft's Cut screen.
5. **A robust cut library.** Every job sent from CutCraft is kept. The user curates it in folders
   and subfolders. CutCraft can send a prepared job to the library without cutting it. Each job
   records when it was last cut and has notes the user can edit.
6. **Cut from the library on the web**, without opening CutCraft or preparing the job again.
7. **Each job keeps its `.cutcraft` project.** CutCraft gets **File ▸ Open From cameo-pi**; the web
   interface offers the original design and the cut lines (as SVG) for download, to edit in
   VectorCraft or elsewhere.
8. **Backup and restore** of the library, because micro-SD cards fail: an automatic, incremental copy
   to another computer over SFTP, optionally encrypted, restorable onto a new Pi from the web
   interface ([§15](#15-library-backup-and-restore); the ZIP download and rsync of earlier drafts were
   dropped on 2026-10-25). Library only, for every user; the Pi itself is rebuilt, not restored
   (decided).
9. **Mobile first.** Re-cutting a library job will almost always happen on a smartphone, standing
   at the cutter while loading the mat. Then the user walks away and watches the job on the phone.
   Every web page is designed for a phone first and a desktop second.
10. **A fresh mat is the gate to cutting** (decided). A cut starts only on a mat whose loading the
    Pi saw since that cutter's last cut ([§8.2](#82-the-fresh-mat-rule)). It shows someone was just
    at the cutter, and saves them from cutting over a used mat.
11. **The user names the cutters.** The project is called cameo-pi, but each cutter is named on the
    web interface (for example "Workshop Cameo"), and that name is what CutCraft lists.
12. **Several cutters, cutting in parallel.** More than one Cameo can be plugged into one Pi
    (through a USB hub), each job chooses its cutter, and different cutters run different jobs at
    the same time.
13. **Several users, no collisions.** Several CutCraft windows, web sessions and AI tools can use
    the same Pi at once. They never interfere: one cutter runs one job at a time, everyone sees
    the same state, and conflicting or repeated requests get a clear answer and never a second cut
    ([§8](#8-cutting), [§9](#9-requests-and-live-state)).
14. **Users and logins** (decided). cameo-pi supports user accounts:
    - While no user exists, the web interface is open to anyone, with admin rights, for first set-up.
    - Once a user exists, only logged-in users (and programs they approved) have access.
    - Admins create, edit and delete users. Every user can operate the cutters and use the library.
    - Being on the home network is never enough on its own: a compromised device on the LAN
      (a smart plug, say) must not be able to cut or browse the library.

Not wanted (decided): a plugin system, and a camera.

## 3. Users and hardware

- **Users:** the same non-technical hobbyist CutCraft is built for, usually on a MacBook and an
  iPhone, sometimes several people in one household.
- **Server:** a Raspberry Pi Zero 2 W (4 × Cortex-A53, 512 MB RAM, 2.4 GHz Wi-Fi). It has two
  micro-USB (micro-B) ports: **USB** (data, On-The-Go) and **PWR** (power only).
  The installer turns Wi-Fi power saving off (NetworkManager `wifi.powersave = 2`): with it on, the
  Zero 2 W can stop answering for minutes at a time.
- **Cutters:** the original Silhouette Cameo (USB vendor `0b4d`, product `1121`), with a USB-B port.
  The Cameo's USB port is a device port and supplies no power.
- **Wiring:** Pi **USB** port → micro-USB OTG adapter (or an OTG micro-B to USB-B cable) → Cameo.
  For more than one cutter: OTG adapter → a powered USB hub → one cable per Cameo.
  The Pi is powered separately from its **PWR** port with a 5 V 2.5 A supply (the official Pi supply,
  or a good charger), never from a Cameo.
- **Host mode:** set `dtoverlay=dwc2,dr_mode=host` so the data port is always a host, whatever the
  cable says.
- **First check:** `lsusb` on the Pi lists vendor `0b4d`. It sends the cutter nothing.

## 4. Words the user sees

| Word | Means |
|---|---|
| **cameo-pi** | The Pi, as a whole (the web address and the project). |
| **Cutter** | One Cameo plugged into the Pi, under the name the user gave it. |
| **Job** | One thing to cut: a prepared design with its material, mat and placement. |
| **Library** | All jobs on the Pi, in folders. |
| **Queue** | The jobs waiting for one cutter, in order. |
| **Load the mat** | Load it on the Cameo. A fresh mat is needed for each cut ([§8.2](#82-the-fresh-mat-rule)). |
| **Test cut** | As in CutCraft: a small square near the design to check the blade. |
| **User** | Someone who can log in to cameo-pi. |
| **Admin** | A user who can also add, change and remove users. |

Avoid "server", "daemon", "endpoint", "token", "turn", "revision" and "SSH" in the web interface,
except on the backup page's advanced section.

## 5. Architecture

```text
CutCraft (each computer) ──┐
Phone / desktop browser  ──┼── home network (HTTP + live events) ──  cameo-pi (Pi Zero 2 W)
AI tool (MCP)            ──┘                                          │   one Rust program
                                                                      ├── library on disk
                                                                      └── USB ── Cameo(s)
```

- **One Rust program on the Pi** (`cameo-pi`), started at boot. It owns the cutters, the library,
  the queue, the web interface, the MCP service and backups. There is no separate web server, proxy
  or database.
- **CutCraft prepares; the Pi cuts.** Opening files, Make cut-ready and tracing stay in CutCraft:
  they need the VectorCraft engine and far more than 512 MB. The Pi receives the finished job.
- **Reuse CutCraft's light crates**, pinned to a CutCraft commit:
  - `cutcraft-job`: projects, `.cutcraft` files (which embed the original design), materials, mats.
  - `cutcraft-path`: `CutPlan`, `plan_sheet` (planning from a project's prepared geometry),
    `test_cut_plan`, the timing model.
  - `cutcraft-device`: the Cameo driver (`driver::run`, `Controls` for pause/stop, `Progress`,
    `query_status`, the start-up status settling), the GP-GL encoder (`gpgl::encode`) and its stream
    allowlist, the USB transport (`nusb`).
  - `cutcraft-sim`: the Simulated Cameo, for development and the web interface's practice cutter.
  - Never `cutcraft-import`, `cutcraft-control` or the desktop app: they pull in VectorCraft.
  - None of the four depends on `cutcraft-import` or VectorCraft (checked in the manifests).
    That is not proof of an aarch64 build or of acceptable memory use: `cutcraft-path` pulls in
    `vello_cpu`. Milestone 1 measures the binary, its memory, and planning time on the Pi.
- **The Pi is the single authority** for cutters, turns, queue and library. Clients send requests;
  the Pi validates them, applies them in order, and pushes the resulting state to every client that
  is watching. No client keeps its own copy of the queue or a cutter's state.
- **One worker per cutter** owns that cutter's USB handle, mat observation, turn and execution. Every
  request that affects a cutter is decided inside its worker, one at a time, so two requests can
  never both win.
- **The network protocol** is versioned HTTPS + JSON ([§14.4](#144-encryption)), with a live event stream (WebSocket or
  Server-Sent Events) for state and progress. Its types live in one Rust crate used by both sides
  ([§18](#18-open-questions), question 1).
- **Web interface:** plain HTML, CSS and JavaScript embedded in the binary, with no front-end build
  step and nothing fetched from the internet. The cut viewer draws on a canvas or with SVG.
- **Language and checks:** Rust, edition 2024, the same toolchain pin as CutCraft, `unsafe`
  forbidden, the same four cargo checks.

## 6. Cutters

- **Discovery** on the Pi lists USB devices with the Silhouette vendor ID without opening them, as
  CutCraft does. Opening a cutter, and every status query, happens only for a cutter the user has
  set up.
- **Ids:** each cutter has a stable id on the Pi, part of every turn and execution id, so two
  cutters' records never collide.
- **Identity:** each cutter is recognised by its USB port path (bus and port chain). The original
  Cameo reports no serial number through USB, neither listed nor opened (measured on the Pi,
  2026-10-24), so the port is all there is. A cutter that reports a serial is recognised by it
  instead. Never commit a real serial.
  - Moving a cutter to another port makes it look new. When one known cutter of the same model has
    gone and a new one appears, the web interface asks "Is this Workshop Cameo?" with one tap to
    keep its name.
- **Set-up, on the web interface:** a newly plugged-in Cameo shows as "New cutter". The user gives
  it a name; it is then a cutter, listed in CutCraft under that name. The user can rename it, or
  forget it.
- **What the user sees:** Off or unplugged, No mat, Mat loaded, Mat loaded but not seen loading
  ("unload and load it again"), Waiting for a mat for <job>, Ready to start, Test cutting, Cutting,
  Paused, Stopping, Finished, Interrupted, Check the cutter (unload the mat). These are the user-facing names for the states in
  [§8.5](#85-states-and-transitions).
- **Several cutters:** each cutter has its own worker thread and USB handle, so cuts on different
  cutters run in parallel. The Zero 2 W's four cores and USB 2.0 bandwidth are ample. The Cameo
  needs a few kB/s.
- **Simulated Cameo:** a setting adds a practice cutter driven by `cutcraft-sim`, with a virtual
  mat that a button "loads". It behaves like a real cutter everywhere, including in CutCraft, and is
  how the whole flow is developed and tested without moving a blade. It also models the awkward
  cases in [§8.1](#81-seeing-the-mat): the brief false Ready of an unloaded Cameo, a quick
  unload/reload between polls, unplug/replug, and a restart with a mat already loaded.

## 7. The library

### 7.1 On disk

The library is plain folders and files, so moving, renaming and backup are simple and the
library is readable without the program:

```text
library/
  Shirts/
    Summer 2026/
      Planisphere/                one job
        job.json                  changeable details: id, name, notes, current revision,
                                  created, last cut, times cut, recent history
        revisions/
          1/                      one revision, never changed once published
            revision.json         what was sent: cutter model, material, mat, from which device,
                                  planner version, plan_id
            project.cutcraft      the CutCraft project (always stored; embeds the original design)
            plan.json             the CutPlan to cut
            preview.svg           generated by the Pi from the plan
```

- Library folders are real folders. A job is a folder containing `job.json`.
- **Revisions:** what gets cut (project, plan, preview) is immutable once published. Sending an
  updated job, or any re-plan, publishes a new revision; older revisions are kept until the user
  deletes them. A job's notes, name and history are its changeable details, in `job.json`.
- Every job has a permanent id inside `job.json`; its folder name is just its display name. Name
  clashes get " (2)", as a file manager does.
- Nothing else is kept in the library folder; settings, pairings, execution records and logs live
  elsewhere in the data folder ([§16](#16-the-pis-software)).

### 7.2 Writing safely

A job's files are only useful together, so atomic files aren't enough on their own:

- **New job:** build the whole job folder in `.staging/<random>/` inside the data folder, write and
  `fsync` every file, validate it, `rename` it into place, then `fsync` the parent folder. Only then
  is the upload acknowledged.
- **New revision:** stage and publish `revisions/<n>/` the same way, then replace `job.json`.
- **Changing details** (notes, name, history): write a new `job.json` beside it, `fsync`, `rename`
  over the old one, `fsync` the folder.
- **Moves and renames** are a single `rename` within the library.
- **At start-up**, leftover `.staging` folders are deleted. A revision folder that `job.json`
  doesn't mention yet is either completed (if valid) or removed.
- Backups read a consistent snapshot ([§15](#15-library-backup-and-restore)), never a mix of files from
  before and after an edit.

### 7.3 What a job records

- Name (defaults to the design's name), folder, notes (free text, editable).
- Per revision: material, mat, size, estimated time, cut length, number of cuts, mirrored or not,
  colours, and the cutter model it was planned for. A job can be cut on any cutter of that model.
- Created (when and from which device), last cut, times cut.
- History: each cut or test cut with date, cutter, revision, who started it, and how it ended
  (finished, stopped, interrupted). `job.json` keeps the last 50 entries plus the totals. The full
  record is in the Pi's rotated cut log.

### 7.4 Using the library

- **Browse** folders, search by name and notes, and sort by name, last cut or created.
- **Phone-first navigation** (decided 2026-10-25): no text breadcrumbs. Each folder shows a big
  back arrow (up one folder) and its name as the title, with the folders above it in small type.
  Tapping the title opens a sheet to jump to any folder above. ⋯ holds the folder actions (New
  folder, Move this folder, and Delete for an empty folder). A job page has ⋯ too, for Move and Delete. A job page has the same header, with the job's name as the title and a back arrow to its folder.
- **Moving a job** is browsing, not typing: a tall sheet uses the same back arrow and title. Tap
  folders to go in, then **Move here**; the folder you're in is the destination, and the job's own
  folder can't be chosen. **New folder here** makes a folder inside the one shown and goes into it.
- **Moving a folder** uses the same picker and takes everything in it along. It can't go inside
  itself (the folder itself isn't offered), a folder of the same name there makes it `name (2)`,
  and it waits while one of its jobs is queued or on a cutter, like moving that job would.
- **Organise:** new folder, rename, move (job or folder), delete. Deleting a job puts it in
  **Recently deleted** for 30 days.
- **Recently deleted** (decided 2026-10-25) appears as a folder at the library's top level, after the
  folders. It isn't a real folder: it can't be renamed, moved or deleted, and no folder can take its
  name. It lists each deleted job with its thumbnail, the folder it was in, when it was deleted,
  and "Will be permanently deleted on <date>". Tap one for **Restore to <folder>** (the top if
  that folder is gone; a clash makes `name (2)`) or **Delete permanently**. A library replaced by
  a restore is listed too, for Delete permanently only. Anything older than 30 days is deleted
  hourly. A job records the folder it came from when it's deleted. A folder must be empty to delete. A job can't be deleted
  or moved while it is queued or cutting.
- **Open a job:** preview, details, notes, history, and **Cut**, **Test cut**, **Download design**,
  **Download cut lines (SVG)**, **Move**, **Delete**.
- **Download design** extracts the original design kept in the `.cutcraft` project in its own format
  (SVG, PDF, PSD, …). **Download cut lines (SVG)** always exists: the cut-ready shapes at their
  real size. Both are downloads only ([§14.3](#143-browser-protections)).
- **Concurrent edits:** every change carries the revision of the details it was based on. If someone
  else changed the job meanwhile, the change is refused with "This job was changed on <device>; here
  is the current version", never silently overwritten. Notes being edited by two people at once is
  the typical case.

## 8. Cutting

### 8.1 Seeing the mat

The Pi can only know what the Cameo's status replies show, and they need care. CutCraft's driver
found that an unloaded Cameo can answer Ready for about a second after a firmware query, and it
requires several seconds of uninterrupted Ready before cutting (`SETTLE_S` in
`crates/device/src/driver.rs`).

- **Polling:** while idle, each cutter's worker sends a status query (`ESC ENQ`: ready / moving / no
  media) at a candidate rate of every 0.5 s. Nothing else is sent while idle.
  - The rate is a candidate, not a finding. Milestone 1 measures, with the developer's go-ahead, that
    hours of polling are harmless and how quickly a load and an unload show. No real cutter is
    polled before then.
  - The poller belongs to the cutter's worker, the single owner of its USB handle. Polling stops
    while the driver runs a cut, so replies can't go to the wrong reader.
  - Each query has a bounded timeout and backs off after errors. Polls never queue up, and never
    delay a Stop.
- **No mat** is believed after two No-mat replies in a row. One reply can be stale or transient.
- **Mat loaded** is believed after Ready without a break for `SETTLE_S`.
- **A load is seen** only when *No mat* was believed and then *Mat loaded* was believed, with no
  gap in polling between them. A gap is no successful reply for longer than 2 s, a USB
  reconnect, or a restart.
- **A gap breaks the evidence at once:** a half-observed chain is discarded, and a Fresh mat
  becomes Unseen and can't authorise a Start. Used stays Used and never becomes fresh again without
  a new, complete observation. Ordinary Ready polls on a mat that is already Fresh change
  nothing.
- **Each seen load starts a new mat generation**, a counter per cutter. Requests to start a cut name
  the generation they were made for ([§9.2](#92-start-is-bound-to-what-the-user-saw)).
- **Anything not seen is unknown, never fresh.** A mat already loaded at start-up or reconnect, and
  Ready after a gap, leave the mat *not seen loading*. The user is told plainly: "cameo-pi didn't
  see this mat being loaded. Unload it and load it again."
- **The guarantee is about observed changes only.** A reload so quick that no poll saw the
  No-mat state is invisible: the Pi can't mark it, and treats the mat as the same one. Milestone 1
  measures how quick a reload has to be to go unseen.
- **Seeing a load shows someone was at the cutter when it was loaded.** It doesn't prove that
  whoever presses Start is there, nor that new material was put on. The loading steps still say
  "fresh material", as in CutCraft.

### 8.2 The fresh-mat rule

- A mat is **fresh** when its load was seen ([§8.1](#81-seeing-the-mat)) and it belongs to no turn
  yet, or to the current turn, which hasn't run a main cut on it.
- **A main cut needs a fresh mat.** If the mat is used or not seen loading: "Unload the mat, put on
  fresh material, load it, then press Start again." This is CutCraft's "load the material" step,
  enforced on the Pi.
- **Test cuts don't use up a mat.** Repeat test cuts, and the cut after them, run on the same load,
  exactly as CutCraft's steps already say ("leave the mat loaded").
- A **finished** test cut keeps the mat fresh and records its area as taken.
- A **confirmed orderly stop** of a test cut also keeps the mat fresh, and its whole square is
  taken. Stopping a test to change the blade is normal, and no extra successful test is required
  afterwards. A stop counts as confirmed only when all of these hold:
  - the driver reports `Stopped { idle: true }`;
  - the connection was never broken;
  - status then settles to Mat loaded again (§8.1). The driver's `idle` also covers No mat, so
    `idle` alone isn't enough.
- **Anything less is a fault.** An unconfirmed stop (`idle: false`), No mat, a lost connection, or an
  **interrupted** test (USB lost, cutter off) all mean the Cameo may have lost its place. The mat
  becomes Used and the cutter shows **Check the cutter**. Nothing is cut there until the Pi has seen
  the mat unloaded (No mat believed).
- Hardware acceptance checks that, after an orderly Stop, a following test cut and main cut land in
  the right places.
- **Taken areas** (test squares) are kept for the turn. Later test cuts and the main cut must not
  overlap them; the Pi checks this as well as CutCraft.
- A main cut that finished, was stopped or was interrupted uses up the mat.
- Ending a turn in any way (main cut, Release, Skip, Remove) retires the mat generation it took,
  even if nothing was cut. A turn that never took a mat has nothing to retire. A mat generation
  belongs to one turn at most.
- The rule is the same whoever asks: CutCraft, the web interface or MCP.

- **"It has fresh material"** (decided 2026-10-25). A mat that's loaded but wasn't seen loading
  can be declared fresh by a person, on the cutter's page or through MCP.
  - **When:** after the Pi restarts or reconnects, the mat is still there, but its load wasn't
    observed. The person knows it holds fresh material and shouldn't have to reload it.
  - **Effect:** it then counts as freshly loaded: a new generation, and Start is allowed.
  - **Recorded:** the cutter's page shows who confirmed it.
  - **Never:** for a mat the Pi knows was cut on (used), and not while the cutter is busy or
    needs checking.
  - **The message** reads "Currently loaded mat may contain previously cut material: unload it and
    load it again with fresh material".

### 8.3 Turns

A **turn** is one job revision's use of one cutter. Each turn is a separate instance with its own id:
the same job queued twice is two turns. It covers waiting for a mat, any test cuts, and the main
cut. Holding the whole sequence matters: no other job may run between a test cut and the cut it was
testing.

- **Starting:** a turn starts with the first test cut or main cut asked for on a free cutter, or
  when a queued job reaches the front ([§8.4](#84-the-queue)). It takes the current mat if that is
  fresh and belongs to no turn; otherwise it waits for a fresh mat.
- **A test cut never ends a turn**, whatever its outcome.
- **A turn ends** when its main cut finishes, is stopped or is interrupted, or when someone
  **releases** it ("Done with this cutter", or Remove/Skip on the web).
- **Who may act on a turn (decided):** no device or browser connection owns a turn. Any authorised
  client can start, test, release or stop the current turn, so a sleeping phone or a closed laptop
  never strands it. Authorised means a logged-in user or a program a user approved, or anyone
  while no users exist ([§14.1](#141-users-and-open-mode)). The usual stale-request checks still apply ([§9.2](#92-start-is-bound-to-what-the-user-saw)).
  The device that queued it is recorded and shown, and every action is attributed.
- **Abandoned turns** are never skipped silently. The web interface shows them as waiting, and
  anyone can Skip or Remove them. Skip sends the job to the end of the queue. Whoever queued
  the job sees what happened, and who did it.

### 8.4 The queue

- **A busy cutter queues.** Asking to cut on a cutter that has a turn adds the job to that cutter's
  queue. Nobody has to keep checking: the requester sees "2nd in line for Workshop Cameo, about
  12 min", and the job comes up as soon as the cutter is free. CutCraft's Cut screen shows the place
  in line, then the usual steps and progress.
- **The front of the queue still needs a fresh mat and Start.** A queue never starts a cut on its own:
  each job needs its own material loaded.
- **One cutter per job (proposed):** a job is queued for a chosen cutter. Whether to offer "any free
  cutter" is open ([§18](#18-open-questions), question 4).

### 8.5 States and transitions

Each cutter's worker keeps:
- the **connection**: Offline or Online, with a connection epoch that changes on every reconnect;
- the **mat**: None, Fresh(generation), Used(generation), or Unseen;
- the **activity**: Idle, Testing, Cutting, Paused, Stopping, or Fault ("Check the cutter");
- the current **turn**, if there is one, with an **action epoch** ([§9.2](#92-start-is-bound-to-what-the-user-saw));
- the queue.

Every row below happens atomically inside the worker. **Ready to run** means:
- the connection is Online and the activity Idle;
- the request names the current turn (or asks for a new one on a free cutter), its revision, the
  mat generation and the action epoch;
- the mat is Fresh(that generation), taken by no other turn;
- for a main cut, no main cut has run on it yet;
- the cut avoids the taken areas.

| When | Condition | Effect |
|---|---|---|
| Cutter appears or reappears | | Online, new connection epoch, mat Unseen, activity Idle |
| Cutter vanishes | any | Offline; mat evidence discarded; a running execution is Interrupted (uncertain); during Stopping, the stop is *not* declared successful |
| No mat believed (§8.1) | Idle or Fault | mat None; Fault clears to Idle |
| Mat loaded believed after None, no gap | Idle | mat Fresh(new generation) |
| Mat loaded believed with no complete chain | Idle | mat Unseen (Used stays Used) |
| Poll gap > 2 s | Idle | pending chain discarded; Fresh → Unseen; Used and None stay |
| Test cut asked | ready to run | execution record written; turn takes the mat; activity Testing |
| Test finished, or confirmed orderly stop (§8.2) | | mat stays Fresh; area taken; activity Idle; turn continues |
| Test stopped without confirmation, interrupted, or lost media | | mat Used; activity Fault; turn continues, waiting for a mat |
| Main cut asked | ready to run, no main cut yet | execution record written; activity Cutting |
| Pause / Resume | Cutting or Paused | Paused / Cutting (between cuts only, as in the driver) |
| Stop | Testing, Cutting or Paused | action epoch advances; Stopping; then one terminal outcome |
| Stop | Stopping | returns the current stop and execution; nothing changes |
| Main cut completed, or a confirmed orderly stop | | mat Used; turn ends; history recorded; next queued turn starts |
| Main cut stopped without confirmation, or interrupted | | mat Used; activity Fault; turn ends; history says uncertain |
| Driver outcome `Refused` (validation, no motion) | | nothing cut; not counted as a cut; mat and turn unchanged; the reason shown |
| Driver outcome `NotLoaded` | | nothing cut; mat Unseen; turn waits for a mat |
| Release / Remove / Skip | turn present, activity Idle | turn ends; its mat generation retired (Used); Skip requeues the job at the end |
| Release / Remove / Skip | Testing, Cutting, Paused or Stopping | refused: "Stop the cut first" |
| Any other request | condition not met | refused with the reason and the current state; nothing changes |

- **One terminal outcome per execution.** Completion and Stop race inside the worker, and the first
  terminal outcome recorded is final.
- Every callback and progress event carries its execution id. A late one for an execution that has
  already ended is ignored, so it can never change the next turn.
- Every row that changes the turn, the mat, the activity or the connection advances the action
  epoch.

### 8.6 Progress and controls

- The driver's progress (phase, share done, time left, estimated blade position, cuts done) streams
  to every client watching that cutter, from the moment a cut starts until it ends.
- **Pause, Resume and Stop** work from any paired client and the web interface, independently of the
  turn. Stop always wins.
- The job runs on the Pi, so a client closing, sleeping or losing Wi-Fi changes nothing. It sees the
  current state when it reconnects ([§9.3](#93-live-state)).
- **Interrupted** (cutter unplugged or switched off, USB error) is reported as in CutCraft: what was
  cut is uncertain. Never resume halfway; load fresh material and cut again.
- **Notifications** ([§18](#18-open-questions), question 6): at minimum the web page shows the end
  of a cut; phone notifications are worth considering.

### 8.7 Crash and power-loss recovery

- **Before the first possible motion of any execution, test or main,** the worker writes and
  `fsync`s an execution record: id, cutter, connection epoch, turn, job revision, test or main, the
  request's key, start time. When the cut ends, the
  record gets its outcome.
- **At start-up**, any execution record without an outcome is marked *interrupted, what was cut is
  uncertain*. It shows in the job's history and on the cutter. It is **never replayed**.
- A crash after the cutter finished but before the outcome reached disk therefore shows as
  "interrupted": a wrong but safe answer. It can never become a second cut.
- After start-up every mat is Unseen, and turns that were waiting are restored from the queue file.
  Turns that were cutting are ended.

## 9. Requests and live state

### 9.1 Every change is asked for once

- Every request that changes something (upload, queue, start, test, release, edit, delete, restore)
  carries an **idempotency key** chosen by the client for that one user action.
- The Pi keeps each key's result. Keys for starts are kept durably with the execution record; the
  others are kept for at least 24 hours. A repeated request with the same key returns the original
  result (the same queue entry, execution or revision). It is never applied twice.
- So a phone that sleeps mid-request, or CutCraft reconnecting and retrying, can't queue twice or
  cut twice. Users never see the keys.

### 9.2 Start is bound to what the user saw

A Start (or test cut) names the server **boot** it was made under, the cutter, the turn (or "a new
turn"), the mat generation, the exact job revision, and the turn's current **action epoch**. A
request from before a restart is refused outright, whatever its counters say, because the
counters start again at each boot. The worker checks them all, atomically,
against its current state.

The action epoch is a single-use authorisation. It advances with every change to the turn, mat,
activity or connection, including every Stop and every finished execution. A delayed main Start
made before a test was stopped therefore fails, even though the turn, revision and generation are
unchanged. Epochs, turn ids and execution ids are never reused across restarts: they include the
boot id. The checks:

- the turn was skipped or removed → refused;
- the job has a newer revision than the one on the page → refused ("this job was updated; check it
  again");
- the mat was unloaded or replaced since → refused ("load a fresh mat");
- the action epoch is not the current one → refused ("things changed; check the cutter again");
- another Start already won → refused with the running execution. With the same idempotency key it
  returns that execution instead.
- The same idempotency key with a different action or body is refused, never answered with the
  earlier result.

A delayed Start from an old page, or a Start arriving after a Stop, therefore can't cut. Tests
cover a retried Start, a never-before-seen delayed Start, and a reused key with a different
body.

**When a job gets a new revision,** every turn and queued entry for it moves to the new revision,
and the action epoch advances. A queue always cuts the job as it is now. A cut already running
finishes the revision it started with, and history records that revision. The cutter page shows
the loading steps, material and preview of exactly the revision its turn holds. A page still
showing the old revision can't start anything, and Queue names the revision the person was
looking at, so it is refused if the job changed since.

### 9.3 Live state

- Each event carries the Pi's boot id and a sequence number. A client that connects, reconnects,
  or sees a gap in the numbers fetches a full snapshot, then continues from it.
- Events are observations (progress, state changes), never commands. Nothing is replayed from them.

### 9.4 Two at once

Two clients pressing Start together: the worker takes them in order, the first wins, the second
gets "already cutting <job>, started from <device>" and switches to watching it. Two clients queuing
the same job: two queue entries, unless it's the same idempotency key.

## 10. Uploading jobs

A job arriving over the network is a claim from a client. One validation path applies to every
upload, from CutCraft, the web interface or MCP, before anything is stored as cuttable:

- **Format:** a `.cutcraft` project plus its `CutPlan`, never raw GP-GL or another device stream.
- **Limits:** upload and decoded size, number of operations and points, project size; refused
  before parsing more than needed.
- **Schema:** supported project and plan schema versions. `ProjectFile::open` already refuses newer
  project formats; the same applies to plans.
- **Agreement:** the plan's sheet and project revision exist in the project, and its
  `source_sha256` matches the project's embedded design.
- **Physical checks:** every coordinate is finite and inside the cutting area of the job's mat,
  for the actual connected cutter model, including each operation's `tool_path` (the blade's real
  path). Settings are within the model's ranges. `gpgl::encode` succeeds, and its stream allowlist
  passes as a final check.
- **The Pi works out** `valid`, the diagnostics, bounds and estimate itself. It ignores the
  client's values, including the supplied `plan_id`.
- **The metadata must agree with the paths.** The operation settings and compensation, and the
  material and mat shown to the user, must agree with the plan, so a physically valid plan can't
  come with misleading loading steps. Execution semantics the Pi doesn't support (an unknown
  operation kind or setting) are refused, even if every coordinate is in bounds.
- **Which plan is cut (decided): the uploaded plan**, the one the user
  reviewed in CutCraft. It is immutable from upload on.
  - This guarantees **physical validation of the reviewed plan**, not proof that the plan was
    correctly derived from its project. These are different guarantees, and the spec doesn't claim
    the second.
  - **Identity:** the Pi computes an execution digest from the plan's operations, tool paths,
    settings, compensation, mat, limits and repeats. CutCraft's private `plan_id` becomes a shared,
    versioned identity function, or the protocol defines a separately named digest. Start, the
    execution record and the preview all use that digest. The preview is generated from the same
    validated values. A digest proves identity, not safe derivation.
  - **Same planner version:** the Pi regenerates the plan with `plan_sheet` and compares the
    submitted contents strictly with the regenerated plan. A mismatch is refused, never excused by
    falling back to the physical checks.
  - **Different, compatible planner version:** accepted under an explicit compatibility policy (the
    versions and semantics the Pi supports), recorded as "derivation not independently verified".
    An unknown version is refused.
  - The plan is never adjusted silently, and regenerated paths are never substituted for the
    reviewed ones.
- Anything refused is refused before it is stored or sent, with a message CutCraft can show.

## 11. CutCraft integration

Changes in the CutCraft repository, each with MCP parity (CutCraft's rule):

- **Discovery:** CutCraft browses Bonjour (`_cameo-pi._tcp`) and lists each cutter on each Pi as
  "<cutter name> (on <Pi name>)" beside USB cutters. A Pi can also be added by address.
- **Pairing:** the first connection asks the Pi for access; the user approves it on the Pi's web
  interface, logged in once users exist ([§14](#14-users-pairing-and-browser-safety)). After that,
  nothing to type.
- **Cut screen:** unchanged. Cut and Test cut send the job (if not already in the library) and run it
  on the chosen remote cutter. The loading steps, the "It's loaded — Start" step and progress work as
  for USB. Added states: "In line: 2nd for Workshop Cameo" and "Load a fresh mat".
- **Send to cameo-pi** (on the Cut screen, and File menu): stores the prepared job in the library,
  choosing a folder, without cutting.
- **File ▸ Open From cameo-pi…:** browse the library and open a job's `.cutcraft` project, to
  re-prepare it or change it.
- **Materials:** CutCraft sends its material and mat lists to the Pi automatically (on pairing, when
  it looks at the Pi, before sending, and after a material or mat changes), so the web interface
  can offer them in Adjust (§12.1). The Pi keeps the last lists it received and never edits them.
- **Settings drift:** a library job cuts with the settings of its revision. If the material was
  changed since, the web interface says so. Using the new settings makes a new revision
  ([§17](#17-versions-and-compatibility)).

## 12. Web interface

Mobile first: designed for a phone held in one hand at the cutter, then widened for a desktop.
Large touch targets, no hover-only controls, readable in light and dark, and fast on a phone over
2.4 GHz Wi-Fi. Pages:

- **Cutters** (home): each cutter with its state, the job on it, progress and time left, and its queue.
  Tap a cutter for the live viewer and controls.
- **Live viewer:** the mat to scale, the cut lines in cut order, moves between cuts, taken test
  areas, the part already cut, the blade's estimated position, time left, Pause / Resume / Stop.
  It keeps updating while the phone's screen is on, and catches up when the page is reopened.
- **Library:** folders, search across the whole library (name and notes), sort by name, last cut or
  newest, jobs with preview, last cut and notes, and Recently deleted ([§7.4](#74-using-the-library)).
  From a job: **Cut** (choose the cutter if more than one), **Test cut**, **Adjust**
  ([§12.1](#121-adjust)), downloads, and ⋯ for move and delete.
- **Cutting from the phone:** pick a job → Cut → "Load a fresh mat on Workshop Cameo", showing where
  the material goes, which side down and the blade setting, the same steps as CutCraft's → the Pi
  sees the mat load → **Start** → live viewer.
- **Settings:** in sections that fold, one open at a time: the Pi's name; users; programs (paired
  CutCraft computers and AI tools); library backup ([§15](#15-library-backup-and-restore)); about.
- **Log in** (once users exist): user name and password, with "stay logged in on this phone" on by
  default ([§14.1](#141-users-and-open-mode)).
- **Users** (admins): add, edit, reset password, remove; see and end a user's sessions and programs.
- **Open-mode banner** while no user exists: "Anyone on your network can use cameo-pi. Add a user to
  require a login." with an **Add a user** button.

### 12.1 Adjust

(Decided 2026-10-25.) A library job's cut settings can be changed on the Pi, for cutting the same
design again on different material, at another size or somewhere else on the mat. The design, and
how Make cut-ready prepared it, change only in CutCraft.

- **On the phone (Adjust, from the job page):**
  - **Size and place:** size (a width, aspect kept, or back to the original), position (a corner
    or the middle of the piece or the mat's sticky area, then ⅛ in nudges), rotation in 90°
    steps, and mirror.
  - **Material:** a material from CutCraft's list, then blade, force, speed, passes and overcut.
  - **Mat and piece:** the mat (from CutCraft's list or the standard mats), and the size of the
    piece of material.
  - **Weeding:** the weeding box and its margin, and registration (layering) marks.
- **CutCraft only:** which colours or layers are cut and how they're grouped, weed lines and
  invert, the test spot, blade-offset calibration, the machine profile, and the design and its
  preparation.
- **How it works:**
  - The Pi applies the changes to the revision's project and plans it with its own planner (CutCraft's
    `plan_sheet`). Planning a real design takes well under a second (§19, milestone 1 step 7).
  - Every change previews on the mat first, with the problems if it can't be cut.
  - **Save** (or **Save and cut**) makes the job's next revision. It goes through the upload
    check (§10), is recorded as verified, since the Pi planned it, and remembers which revision it
    was adjusted from. The earlier revision stays.
  - **Refusals:** an adjustment of a revision that is no longer current is refused, and so is one
    that can't be cut.
- **Material and mat lists** (§11): CutCraft sends its lists automatically.
  - **One list per computer:** the Pi keeps each CutCraft's lists apart, by the access key that
    sent them, and offers them together.
  - **Shared names:** when two computers have a material or mat of the same name, the most
    recently sent version wins, and Adjust says which computer each material came from.
  - **Updates and removal:** a computer's new lists replace only its own, and they go when its
    key is removed.
  - The Pi never edits them: a change on the phone changes that job only.
  - Without the lists, the job's own material can still be changed.
  - When the job's material has different settings in the list now, the job page says so and
    offers **Use the current settings** (settings drift), which makes a new revision.
- **The same in the protocol** (`/api/v1/jobs/{id}/adjust`, `/adjust/preview`,
  `/api/v1/catalog`) **and MCP** (`catalog`, `job_adjust_preview`, `job_adjust`).

## 13. MCP on the Pi

- An MCP service (Streamable HTTP, `/mcp`) offering what the web interface offers:
  - list cutters and their state
  - browse, search and edit the library
  - send a job (a `.cutcraft` project plus plan)
  - queue and start a cut or test cut
  - pause, resume, stop
  - watch progress
  - see the backup's state and back up now
- It follows the same rules: one validation path, turns, the queue, the fresh-mat rule, idempotency
  keys and bound starts.
- It uses the same access keys as CutCraft ([§14](#14-users-pairing-and-browser-safety)). Once users
  exist, an AI tool needs a key approved by a logged-in user, and acts as that user. Actions are
  marked with the user and the client that did them.
- Follow CutCraft's MCP conventions (`docs/mcp.md` in the CutCraft repo) for tool naming, errors and
  annotations.
- **As built (0.13.0, extended through 0.20.0):**
  - **Endpoint:** `/mcp`, behind the same guard as the protocol. A key is needed for anything,
    and requests carrying a browser `Origin` are refused.
  - **Keys:** an AI tool's key is made on the web page and shown once (§14.2's fallback).
  - **Tools:**
    - **Cutters:** `cutters`, `cutter_wait`, `cut_queue`, `cut_start` (bound, like the
      protocol's), `cut_pause`, `cut_resume`, `cut_stop`, `turn_release`, `mat_confirm_fresh`,
      `practice_mat`.
    - **Library:** `library_list`, `library_search`, `job_get`, `job_file`, `job_send`,
      `job_edit`, `job_move`, `job_delete`, `folder_create`, `folder_move`, `folder_rename`,
      `folder_delete`, `deleted_list`, `deleted_restore`, `deleted_purge`.
    - **Adjust:** `catalog`, `job_adjust_preview`, `job_adjust`.
    - **Backup:** `backup_status`, `backup_now`.
  - **Web only (decided 2026-10-25):** setting up the backup (the computer, sign-in, accepting its
    identity, the key, encryption) and Restore; users; program keys. They decide who and what can
    reach the Pi and where the library goes, so an AI tool doesn't change them. It can see the
    backup's state and run one.

## 14. Users, pairing and browser safety

### 14.1 Users and open mode

- **Open mode (no users yet):** anyone who can reach the web interface has admin rights, so a fresh
  Pi can be set up without a password. The banner in [§12](#12-web-interface) says so on every
  page. Programs still need an access key ([§14.2](#142-pairing-a-program)), approved by anyone.
- **Adding the first user** ends open mode, in one atomic step. The first user is always an admin.
  - The same step lists the programs approved during open mode, each with **Keep** (it then
    belongs to this admin) or **Remove**. Nobody unknown keeps access by accident.
  - The creating browser gets a new session id. Every other browser session, event stream and MCP
    session from open mode is closed.
  - Two first-user requests at once can't both win: the second is refused because users now exist.
- **Damaged account data fails closed.** If the user store can't be read, the Pi refuses all
  access (except the command line, [below](#recovery)) rather than treating it as "no users",
  which would reopen open mode.
- **With users:**
  - Every page, every request, the event stream and MCP need a logged-in user or a program key
    approved by one.
  - The only exceptions are the login page and its static files, plus **pairing bootstrap**
    ([§14.2](#142-pairing-a-program)): asking for access, and collecting an approved key with its
    request secret. Neither gives any access to cutters or the library. Both are rate-limited, with
    a cap on outstanding requests and a short lifetime.
  - Bonjour still advertises the Pi (that reveals only its name), but nothing else answers without
    login.
- **Authorisation is checked on every action**, against the current state of the user, not a role
  remembered from login. A role change, password reset or removal takes effect on existing
  sessions and program connections, including long-lived event streams and MCP sessions, at their
  next action. A cut already running continues regardless, as everywhere else.
- **Roles:**
  - **Admin**: add, edit and remove users, reset passwords, set a user's role, end sessions, remove
    any program key, and the Pi's settings.
  - **User**: everything else: cutters, turns, queue, library, downloads, **library backup and
    restore** (including the scheduled backup's key, target and schedule, [§15](#15-library-backup-and-restore)),
    their own password, and approving their own programs.
  - The last admin can't be removed or demoted, so the Pi can't be locked out from inside.
- **Passwords:**
  - At least 8 characters; no other rules.
  - Stored only as Argon2id hashes, never logged.
  - Admins set a user's first password or a new one. Until the user changes it, that session can
    only change the password or log out.
  - Argon2id's cost is chosen by measuring it on the Pi. Hashing runs outside the cutter workers,
    with a bound on concurrent hashes and on the login queue. A login flood can't starve a Stop or
    use up memory.
- **Login protection:** a slowing delay after failed attempts per user name and per network address,
  and no hint whether the name or the password was wrong.
  - The network address is the connection's own. `X-Forwarded-For` is ignored, because a caller
    could choose its own address with it. Behind the developer's own reverse proxy, every client
    therefore shares the proxy's address and its delay: a deliberate tradeoff, since trusting a
    proxy is out of scope.
- **Sessions:**
  - A random session id in a `Secure`, `HttpOnly`, `SameSite=Strict` cookie, stored hashed.
  - Phones stay logged in for 90 days by default (mobile first); a shared computer can log out.
  - Changing a password, or an admin removing a user, ends that user's sessions and program keys.
    Each user has a credentials value in the account file that is new at every password set or
    reset, and each key records the value it was approved under. A key works only while they
    match, so a reset from the command line ends keys in the running server too. A key also acts
    under its user's must-change rule.
- **Who did what:** every action records the user and the device, browser or program, and the queue
  and history show it ("Queued by Sam from the kitchen MacBook"). In open mode, a browser picks a
  device name the first time it changes something ("This phone is: ___").
- **Every user may pause or stop any cut.** Removing or skipping someone else's queued job is allowed
  but recorded and shown to them.
- <a id="recovery"></a>**Forgotten admin password (decided): command line only.** Run
  `sudo cameo-pi reset-admin` on the Pi (locally or over SSH) to set a new password for an admin. It
  ends that admin's sessions and program keys. There is no web recovery page, no boot-partition
  trigger and no recovery token. An installation with users never goes back to open mode.
- **Passwords and session cookies cross the network,** so everything is encrypted
  ([§14.4](#144-encryption)): a compromised device that knows the Wi-Fi password must not be able
  to capture them.

### 14.2 Pairing a program

OctoPrint "application keys" style:

1. The program asks for access with its name ("CutCraft on the kitchen MacBook"). The Pi returns
   a random **request secret** (128 bits) and a short code.
2. The web interface shows "Allow CutCraft on the kitchen MacBook? Code 4821" with **Allow /
   Deny**. The program shows the same code. The code only tells the user which waiting request
   they're approving; it isn't a security check of the connection.
3. The program collects its key using its request secret. Only that secret can collect it.
   Requests expire after 10 minutes, and each key is handed out once.
4. Keys are stored hashed. They can be renamed and removed one by one. There's also a fallback of
   copying a key shown once on the web interface.
5. **Once users exist**, approving needs a logged-in user, and the key belongs to that user. The
   program then acts as that user, and the key ends with that user's removal or password
   change.

### 14.3 Browser protections

These protections apply in both modes. They stop an unrelated web page, visited on the same
network, from using the Pi, and in open mode they are the only protection:

- **Host check:** requests must name the Pi (`cameo-pi.local`, its set name, or its own IP
  addresses). Anything else is refused, which blocks DNS-rebinding attacks. CutCraft's MCP endpoint
  has a local reference for Host/Origin checks.
- **Origin check and anti-CSRF token** on every changing request from a browser: the Origin must
  be the Pi itself, and the page sends a per-session token (from a `SameSite=Strict` cookie,
  echoed in a header) that another site can't read.
- **Programs** authenticate with their access key in a header, never a cookie.
- **Files and content:**
  - Downloads of original designs and cut lines are sent as attachments
    (`Content-Disposition: attachment`, `X-Content-Type-Options: nosniff`), never displayed in the
    Pi's own pages.
  - Previews are drawn by the Pi from the plan, never from uploaded SVG.
  - App pages carry a strict Content-Security-Policy.
- **Names and paths:** every folder and job name is checked: no `/`, `\`, `..`, control
  characters, leading dot, or over-long names. Every path is resolved inside the data folder.
  Symlinks are never followed out of it.
- Forwarded-address headers are ignored.

### 14.4 Encryption

(Decided.) Like a home router: the Pi serves the web interface, the protocol and MCP over **HTTPS
only**, with a **self-signed certificate** it makes itself. The app does nothing more with
certificates.

- **The certificate:** made at first start for `cameo-pi.local`, the Pi's set name and its addresses,
  and valid for 10 years. It's kept with the Pi's private settings, never in the library or its
  backups, and is never uploaded, renewed or managed in the interface. A rebuilt Pi makes a new
  one, and browsers and programs trust it afresh.
- **Plain HTTP** on port 80 only redirects to HTTPS. Nothing else answers there.
- **First use is trusted, plainly stated.**
  - A browser shows the usual warning the first time, which the user accepts; the setup guide
    explains this in one screen.
  - A program pins the certificate it sees when it first pairs, and trusts exactly that one
    afterwards. A changed certificate stops the program with a plain message ("cameo-pi's
    identity changed; pair again"), never a silent re-trust.
  - As with any self-signed device, this first connection is trusted, not proven. For anyone who
    wants to check, `cameo-pi fingerprint` on the Pi (over SSH) prints the certificate's
    fingerprint, and CutCraft shows the fingerprint it pinned.
- **Out of scope:** reverse-proxy support, certificate upload or renewal, and public certificates.
  Anyone may put the Pi behind their own infrastructure, but cameo-pi doesn't implement or promise
  that integration.
- Session cookies are `Secure`.

## 15. Library backup and restore

(Decided 2026-10-25, replacing the ZIP download and upload.) The point is that a dead SD card
never takes the designs with it. People come to rely on the Pi's library as the only copy of
their designs, so the backup must be easy to set up, automatic, and restorable onto a new Pi in
one step.

- **What's backed up:** the library only: jobs, revisions (projects with their embedded designs,
  plans and previews), notes, history and folders. Never users, password hashes, program keys,
  sessions, the HTTPS certificate, the Pi's settings, cutter names or the queue.
- **Where:** another computer, over **SFTP**, which every SSH server speaks: macOS Remote Login,
  Windows' OpenSSH server, Linux and NAS boxes.
  - Implemented with `russh` and `russh-sftp`. Nothing to install on the Pi or the other
    computer.
  - A ZIP download through the browser doesn't scale to multi-GB libraries over the Zero's Wi-Fi,
    so there is none.
- **The card** (Settings ▸ Library backup):
  - **Where:** the computer (name or address), port, user and folder.
  - **Sign in** with the Pi's own backup key (Ed25519, made on the Pi; only its public half is
    shown, to add to that user's `authorized_keys`), or with a password, kept in the Pi's private
    state.
  - **The other computer's identity:** shown on the first connection and accepted by the person.
    It's checked every time after: a changed identity is refused until forgotten on the card.
  - **Test connection:** signs in, makes the folder if it's missing, checks it can be written, and
    reports free space.
  - **Encryption** (optional), **how often**, **Back up now** and **Restore**.
  - **Setup guides** for Mac, Windows, Linux and NAS, the key, and recovering without a Pi, in a
    dialog of their own.
- **The folder:**
  ```text
  cameo-pi-backup.json       ownership marker, written before the first upload
  commits/<generation>.json  append-only publication record: manifest and key paths
  generations/<id>/manifest.json  authoritative manifest (manifest.age if encrypted)
  generations/<id>/key.age        encrypted generation's password-locked private key
  generations/<id>/library/…      immutable plain files referenced by manifests
  objects/ab/<random id>          immutable encrypted files
  library/…                      derived browsable plain copy, refreshed after commit
  removed/<date>/library/…        derived browsable retained plain files
  manifest.json                  derived plain manifest, for inspection
  ```
  - **Another Pi's folder** (a marker with another Pi's id) is refused, and so is a folder with
    other things in it. One folder per Pi; no subfolders managed for several Pis.
  - **After a Restore,** the Pi takes over that folder.
- **Incremental:** a run uploads only new and changed files, from the manifest. Revisions never
  change, so after the first backup a run sends little. All authoritative files are immutable,
  including plain files. Files go to a unique `.part` name and are renamed to a new name. The
  append-only commit record is written last, after its manifest, files and key; restore selects
  the newest committed generation. Publication never removes an existing pointer. Failed upload
  or retirement leaves the previous generation intact. Browsable copies and cleanup happen only
  after publication; failure there warns that extra or older copies remain. Restore never reads
  the browsable copy. Empty folders are recorded independently of files. Schema-one backups
  remain readable and migrate on their next successful backup. An encrypted backup whose local
  manifest cache is missing must first be checked with its password using Restore; Cancel then
  retains newer local work while allowing backups to continue.
- **Nothing is deleted straight away:** files that disappear from the Pi remain referenced in the removed area
  and are deleted after 90 days, so a mistake on the Pi, or a failing card, can't wipe the backup.
- **Encryption** (optional, decided):
  - **The format:** `age` (X25519, with an scrypt-locked key). It's the standard format, so a
    backup can be opened with the stock `age` tool, without a Pi.
  - **The key:** turning encryption on makes a key pair. The private half is locked with the
    person's password and stored in its immutable generation as `key.age`. The Pi keeps only the public
    half, so scheduled backups never need the password, and a stolen Pi can't read a backup.
  - **Names:** files are stored under random names, and the encrypted manifest maps them back on
    restore. Restore asks for the password.
  - **Changing the setting** converts the backup in place at the next run. Switching encryption on
    re-sends the library encrypted (removed files too) and deletes the plain copy once the
    encrypted one is complete. A new password re-encrypts active library files to the new key. Retained encrypted files keep
    their earlier immutable key and may need the earlier password for manual recovery. Switching it
    off re-sends plain folders and deletes the encrypted files; the encrypted copies of removed
    files can't be read without the password, so they go (the confirmation says so). The marker
    records the public key, so a new key is noticed.
- **How often:** a few minutes after every change (the default; also at least daily), daily at a
  time, weekly on a day and time, or only Back up now.
  - **Failures:** a clear configuration/access error stops immediately and turns the card red.
    Transient connection failures retry after a minute, doubling to an hour, and stop after a day.
    A stopped cycle stays stopped across restarts and library changes; saving settings, accepting
    an identity, a successful connection test, or Back up now explicitly restarts it. Retrying
    turns the card yellow, and giving up turns it red with a warning. A banner on the
    Cutters and Library pages says so, and also while no backup is set up ("Your designs are only
    on this Pi's SD card").
- **Restore:** the whole library at a time.
  - The Pi fetches the backup into staging and checks every file against the manifest. It
    validates every revision as an upload would (§10), with the Pi's own values replacing the
    stored ones.
  - It shows what's there, then Replace (the current library goes to Recently deleted).
  - Replace is refused while any cutter has a turn or a queue, and if backup settings changed
    after fetching. A replacement Pi adopts the backup's encryption key and keeps subsequent
    backups encrypted; prompting depends on the target backup, not this Pi's previous setting.
- **Plain by default:** an unencrypted copy is ordinary folders that anyone with access to that
  computer can open. Each job's `project.cutcraft` opens in CutCraft directly, design included.

### Rebuilding after a failure

The short path, in the setup guide:
1. Flash a new SD card and install cameo-pi.
2. Add users (or use open mode) and pair the programs again; browsers accept the new certificate.
3. Settings ▸ Library backup: the same computer, user and folder. Use a new key added to that
   computer, or the password. Accept its identity, then **Restore from the backup** (with the
   password if encrypted). The Pi then carries on that backup.

Passwords, pairings and settings are not restored, and the guide says so.

## 16. The Pi's software

- **How it's distributed (decided, milestone 8):** three ways in, one package underneath.
  - **The `cameo-pi` package** (`.deb`, arm64, for 64-bit Raspberry Pi OS) holds the program, the
    service, the USB rule, the Wi-Fi setting and the Bonjour file. It also adds cameo-pi's own apt
    repository (on GitHub Pages, signed), so `sudo apt update && sudo apt upgrade` updates it. It
    takes over an install made by the earlier `scripts/pi-install.sh`. Removing it, even purging
    it, never deletes the library.
  - **The install script,** for a Pi that already runs Raspberry Pi OS:
    `curl -fsSL https://mkrzywonski.github.io/cameo-pi/install.sh | sh` installs the package.
  - **The SD-card image,** for someone new to the Pi: Raspberry Pi OS Lite with the package,
    installed at first boot, and the name `cameo-pi`. Raspberry Pi Imager writes it and sets the
    Wi-Fi, the login and the name.
  - **Not Docker:** on 512 MB it costs memory, and it would need the USB device, the host's
    network for Bonjour, ports 80 and 443 and the host's Wi-Fi setting anyway.
  - **Releases:** a version tag builds all of them in GitHub Actions (`.github/workflows/release.yml`):
    the package and the image on GitHub Releases, the repository and the script on GitHub Pages.
    The Pi never builds Rust (512 MB); binaries are cross-built.
- **Wi-Fi and name on the card:** set before first boot (the image's documented boot-partition file,
  or Raspberry Pi Imager's settings), never from the web interface: changing Wi-Fi over that Wi-Fi
  locks people out.
- **Hostname** `cameo-pi.local` by default, with Bonjour (Avahi) advertising both the web interface
  and `_cameo-pi._tcp` for CutCraft.
- **Data folder:** the library, and separately the Pi's private state (users, pairings, settings,
  the HTTPS certificate and key, the backup SSH key, queue, execution records, idempotency results
  and the cut log), on a data partition; everything else read-only or with minimal writes; logs in
  memory.
- **Updates:** no self-updating program: apt updates the package. An update restarts the service,
  which stops a cut in progress, so update between cuts.
- **USB access:** a udev rule gives the program's user access to Silhouette devices; no root.
- **On Raspberry Pi OS,** the package runs cameo-pi as a systemd service
  (`packaging/cameo-pi.service`):
  - It runs as its own `cameo-pi` user, in plugdev for the USB rule.
  - Its data folder is `/var/lib/cameo-pi`.
  - `CAP_NET_BIND_SERVICE` is its only capability, for ports 80 and 443.
  - The filesystem is read-only apart from that data folder, and the unit's other sandboxing
    (no new privileges, the system-service system calls only, no other users' processes) leaves
    devices alone, since the Cameo is a USB device.
  - Files it writes are its own alone (`UMask=0077`); the backup's key, settings and password are
    also set to 0600 explicitly, the password in a file of its own.
  - The package turns Wi-Fi power saving off: with it on, the Zero 2 W stops answering for minutes
    at a time.
  - The site's own options are in `/etc/default/cameo-pi`.
  - Bonjour comes from an Avahi service file (`packaging/cameo-pi.avahi.service`), advertising
    `_cameo-pi._tcp` and `_https._tcp` on port 443.
  - `sudo cameo-pi reset-admin` keeps the data folder's owner on the files it writes.

## 17. Versions and compatibility

- The program, the protocol and the job format each have a version. `job.json`, `revision.json`
  and `plan.json` carry `schema_version` (`CutPlan` already does). Each revision records the
  planner version (the CutCraft commit) that made its plan.
- CutCraft and the Pi state their protocol versions when connecting. If they don't match, CutCraft
  says plainly which one needs updating, never fails obscurely.
- **A revision cuts as reviewed.** A supported migration of an older plan schema must produce
  exactly the same physical behaviour: the tool paths, and also the repeats, the order of tool up
  and down, force, speed, compensation and device setup. Tests compare the encoded streams.
- **Re-planning is a new revision.** If an old revision can't be cut as it is, the Pi may re-plan it
  from its project with `plan_sheet`. The same applies when the user picks newer material
  settings. The result is a new revision with its own preview and settings, shown to the user and
  needing confirmation before Start. The old revision is kept.
- A project too new or too old for the Pi gets an actionable message ("update cameo-pi" / "open it
  in CutCraft and send it again"), never a silently different cut.
- Like CutCraft: semantic versions, a `CHANGELOG.md`, and a version bump with each change to the
  program.

## 18. Open questions

1. ~~Where the shared protocol crate lives~~ **decided:** `crates/station` (`cutcraft-station`) in
   the CutCraft repository.
2. ~~Distribution~~ **decided** (milestone 8): a `.deb` with its own apt repository, an install
   script that installs it, and an SD-card image with it; not Docker ([§16](#16-the-pis-software)).
3. ~~Encryption~~ **decided:** HTTPS only, with a self-signed certificate the Pi makes, trusted on
   first use and pinned by programs; no reverse-proxy or certificate management
   ([§14.4](#144-encryption)).
4. **"Any free cutter":** worth having, or always choose the cutter? It needs the cutters' mats and
   blade settings to match the job. Proposed: choose the cutter at first.
5. **Queue order:** first come, first served; who may reorder it?
6. **Notifications** when a cut ends or needs a mat: web push (needs HTTPS on iPhone), ntfy, or just the
   open page.
7. ~~Re-cutting on another material from the web~~ **decided:** Adjust ([§12.1](#121-adjust)) makes
   a new revision with another material, mat, size, place and more.
8. ~~Placement from the web~~ **decided:** Adjust moves, turns and resizes the design on the mat.
9. **Mat observation on the real Cameo:** is 0.5 s polling harmless, how fast are load and unload
   reported, and is a quick reload ever missed ([§8.1](#81-seeing-the-mat))?
10. **Two Pis:** does CutCraft list cutters from several Pis at once (yes, by design), and should a job
    move between Pis' libraries?
11. ~~Which plan is cut~~ **decided:** the validated, immutable uploaded plan, with strict comparison
    when the planner versions match ([§10](#10-uploading-jobs)).
12. ~~Turn ownership~~ **decided:** any authorised client may act on the current turn, with
    attribution and the stale-request checks ([§8.3](#83-turns)). Authorised means a logged-in user,
    or anyone in open mode ([§14.1](#141-users-and-open-mode)).

## 19. Milestones

1. **Hardware proof.** Every step that talks to the Cameo is announced with its exact command and
   needs the developer's go-ahead. Motion has its own gate. Results stay local (no serials).
   1. **Target build:** a small local probe using only `cutcraft-device`, `cutcraft-job`,
      `cutcraft-path` and `cutcraft-sim`, cross-built on the desktop. It proves the intended
      dependency set. Record the target (musl static is a candidate to verify) and the full CutCraft
      pin. A full static `cutcraft-cli` is optional, and its footprint isn't the server's.
   2. **USB access:** a non-root udev rule (group and mode, since `uaccess` needs a local seat),
      leaving `usblp` loaded. Privileged lines are typed visibly in the shared terminal. Only one
      program owns the USB interface at a time.
   3. **Status observation**, with nothing but status queries. `hardware watch` serves as a
      smoke test only. The probe logs every query's monotonic start and end time, its reply or
      error, and the connection epoch, as evidence for §8.1's rules. It measures load and unload
      latency, and whether a quick reload goes unseen. Open-only observations and observations
      after a firmware query are labelled separately; a firmware query is a separately announced
      step.
   4. **Unplug/replug** as a separate segment: end the run, reconnect, open a new handle, then
      observe. Errors on the old handle are not observations of the new device.
   5. **Bounded long polling:** an authorised run of a stated length (for example 60 minutes)
      recording errors, response latency and any visible activity, stopping on anything unexpected.
      It is evidence for that duration only.
   6. **First motion:**
      - With the blade removed (or a pen the developer approves), a known mat and origin, and a
        small declared envelope well inside the sheet.
      - The motion bounds and exact command are shown first, and the fixed `hardware move` route
        and coordinate frame are reviewed before use.
      - A remembered origin is not enough.
      - Only after this movement and placement check passes: a test pattern, with the tool and
        material confirmed.
      - **Passed 2026-10-24**, with the developer present. A route of tool-up moves alone (`M`
        commands, ending at the start corner) produced no motion, although the driver reported it
        complete. The likely cause is that the firmware defers tool-up moves until a draw command.
        The movement check was therefore an air pass: the 40 mm test-square job, cut from the web
        interface through the server with the holder empty. The pen pass followed, with the same
        job and a reloaded mat. Both finished, and both looked right to the developer.
   7. **Measurements:** the probe's binary size and peak memory (`/usr/bin/time -v` if present;
      nothing installed silently), and `plan_sheet` time and memory on a large public fixture.
      - **Done 2026-10-25** with `cameo-pi-probe plan`, which reads peak memory from
        `/proc/self/status`, because the Pi has no `/usr/bin/time`.
      - **Binaries** (static aarch64 musl): the server is 9.3 MB, the probe 1.9 MB.
      - **Test designs:** authored flower grids, prepared by CutCraft. The Pi ran them next to the
        live service, with about 260 MB free.
      - **Planning time and peak memory on the Pi**, before and after CutCraft 1.2.1's faster
        cut ordering (same plans):

        | Contours (points) | Before | After | Peak memory |
        |---|---|---|---|
        | 325 (5,551) | 0.33 s | 0.05 s | 5 MB |
        | 901 (12,350) | 2.1 s | 0.16 s | 5 MB |
        | 3,601 (35,605) | 23.9 s | 1.5 s | 16 MB |
        | 14,401 (116,843) | 335 s | 27 s | 60 MB |

      - **The full upload check** (which plans again) takes 0.23 s for the first design.
      - **Real T-shirt designs** have tens to a few hundred contours, so re-planning on the Pi
        takes well under a second. Memory is no constraint.
2. **The program, one simulated cutter, with the concurrency foundation:**
   - One worker per cutter, the state table in [§8.5](#85-states-and-transitions), and turns.
   - Mat observation, including the simulator's awkward cases.
   - Idempotency keys, bound starts, execution records and restart recovery.
   - Upload validation, and the atomic library writes.
   - Live events with snapshots.
   - Tests with two clients against the simulator, including simultaneous starts, retried
     requests, a stale start after Stop, and a crash mid-cut.
   - Then the real Cameo.
   - **Done** (0.2.0–0.6.0, 2026-10-24); first real cuts with the developer present on 2026-10-24.
3. **CutCraft as a client:** discovery, pairing, the remote cutter in the cutter list, Cut and Test
   cut with progress, Send to cameo-pi.
   - **Done 2026-10-25** (CutCraft 1.1.1, cameo-pi 0.11.0), as CutCraft spec §5.11 describes.
     It ran end to end against the Pi's practice cutter, and the developer sent a job from the
     window. Not yet: a remote test cut on scrap, and File ▸ Open From cameo-pi (milestone 7).
4. **Library and mobile web interface:** browse, organise, notes, history, cut from the library, live
   viewer, the browser protections.
   - **Done** (0.9.0, 2026-10-24), then reworked on 2026-10-25: Adjust (0.14.0), phone-first
     navigation and folder moves (0.17.0–0.18.0), Recently deleted (0.19.0), search and sort (0.20.0).
5. **Several cutters and the full queue:** the multi-cutter interface, Skip/Remove, a test with two
   CutCraft windows and two phones.
   - **Built** (0.9.0): each cutter's own worker, queue, Skip and Remove, tested with two clients
     against simulators. **Not yet run:** the test with two CutCraft windows and two phones.
6. **Backup and restore.**
   - **Done** as ZIPs and rsync (0.12.0), then **rebuilt** as SFTP backups (0.16.0–0.16.3,
     [§15](#15-library-backup-and-restore)), checked end to end against OpenSSH and run against the
     developer's own computer.
7. **MCP**, File ▸ Open From cameo-pi, downloads.
   - **Done** (cameo-pi 0.13.0, CutCraft 1.2.0, 2026-10-25).
8. **Distribution** and a first-time setup guide for a non-technical user: deliberately left until
   the program worked. Decided and built (0.23.0): the package with its apt repository, the
   install script and the SD-card image ([§16](#16-the-pis-software)), with the guide in the
   README. Waiting for a first release, and a first boot of the image on a Zero 2 W.

## 20. Out of scope

Plugins, cameras, cutting from outside the home network, sign-in through outside accounts (Google,
Apple, …), printers
or other cutter brands, preparing designs on the Pi (opening files, Make cut-ready), roll cutting
until CutCraft supports it on the real Cameo, raw device streams as a job format, backing up or restoring the whole Pi (accounts, keys,
settings), reverse-proxy support and certificate management.

## 21. Revision history

- **Draft 6 (2026-10-25):** decisions while building milestones 3 to 7:
  - Backups: SFTP to another computer, incremental, optionally `age`-encrypted, restorable onto a
    new Pi; the ZIP download and rsync dropped (§15).
  - Adjust on the Pi, with CutCraft's material and mat lists synced (§12.1).
  - The mat override "It has fresh material" for a mat the Pi didn't see loaded (§8.2).
  - Phone-first library navigation, folder moves and renames, Recently deleted, search and sort
    (§7.4); backup setup, users and keys web-only, not over MCP (§13).
  - Distribution left open for comparison (§16, §18).
- **Draft 5 (2026-10-24):** after Codex's review of draft 4 and the developer's decisions:
  - Backups are library-only ZIPs and rsync, for every user. They never include accounts, keys,
    the certificate or settings, and the Pi is rebuilt rather than restored (§15).
  - Admin recovery is command-line only (§14.1).
  - Self-signed HTTPS works like a home router: first use is trusted and stated plainly, with no
    claim that the pairing code checks the connection, no reverse-proxy support, and the
    certificate kept outside backups (§14.2, §14.4).
  - Authentication details (§14.1): pairing bootstrap routes, an atomic first user, failing
    closed on damaged account data, authorisation on every action, a limited must-change session,
    and bounded password hashing.
- **Draft 4 (2026-10-24):** the developer's decisions:
  - Question 11 decided: cut the validated uploaded plan.
  - Question 12 decided, with users (goal 14, §14.1): open mode only until the first user is
    created, logins after that, admin and user roles, keys belonging to users, physical-access
    recovery.
  - Question 3 decided: HTTPS only, with a self-signed certificate pinned by programs at pairing;
    a reverse proxy for browsers that shouldn't warn (§14.4).
- **Draft 3 (2026-10-24):** after the second review round. Changes:
  - Polling made a candidate rate owned by the worker (§8.1). A poll gap turns Fresh into Unseen,
    and the guarantee is stated as covering observed changes only.
  - Confirmed orderly stops versus faults (§8.2), and Release/Skip retiring the turn's mat.
  - Turn instances (§8.3).
  - A full state table (§8.5): preconditions, test execution records, `Refused`/`NotLoaded`
    outcomes, Stop races, one terminal outcome per execution, and the action epoch.
  - Single-use start authorisation, and idempotency-key misuse refused (§9.2).
  - Validated-upload identity and policy (§10).
  - Backups covering metadata, and restore resetting physical state (§15).
  - Migration preserving full physical behaviour (§17).
  - Milestone 1 rewritten as an authorised, staged hardware procedure (§19).
- **Draft 2 (2026-10-24):** after a design review. Changes:
  - Mat observation with an evidence model (§8.1), and the fresh-mat rule made precise, including
    failed test cuts and taken areas (§8.2).
  - Turn start, end and release (§8.3), a state table (§8.5), crash recovery (§8.7).
  - Idempotent requests, bound starts and live-state snapshots (§9).
  - Upload validation (§10).
  - Immutable job revisions and staged atomic writes (§7.1–7.2), bounded history.
  - Browser protections, pairing secrets and content rules (§14).
  - Snapshot backups and safe restore (§15).
  - Faithful compatibility (§17).
  - The concurrency foundation moved to milestone 2 (§19).
- **Draft 1 (2026-10-24):** first version, from the developer's goals.
