# Changelog

Each version of CutCraft-Web and what changed in it. Versions are `major.minor.patch`: patch for
fixes and small changes, minor for new features, major for big changes. Raise it with
`scripts/bump-version.sh patch|minor|major`.

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
