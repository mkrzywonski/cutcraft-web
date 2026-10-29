# Provenance

CutCraft-Web replaces two earlier projects by the same developer, CutCraft and cameo-pi, and
brings their code in (spec §5.2). Their repositories were private when this was imported and will
be deleted. Only the code and its tests came in: no history, no local files, nothing private. Both
were licensed MIT OR Apache-2.0, as this repository is.

## Imported (milestone 1, 2026-10-26)

| Here | From | At | Notes |
|---|---|---|---|
| `crates/job`, `crates/path`, `crates/device`, `crates/sim` | CutCraft `crates/…` | `5462a35` (1.4.2) | Code unchanged, comments edited (pronouns). The code is the same as at `b6aad51` (1.2.1), the commit cameo-pi pinned. |
| `crates/import`, `crates/control`, `crates/station` | CutCraft `crates/…` | `5462a35` (1.4.2) | Help paths in `control` now point at `docs/help/`. |
| `docs/help/mcp.md`, `troubleshooting.md`, `design-guide.md` | CutCraft `docs/` | `5462a35` | Bundled help, included by `crates/control`. |
| `docs/reference/cutcraft-spec.md` | CutCraft `docs/spec.md` | `5462a35` | Reference for the desktop layout (spec §9.2). Not this project's spec. |
| `crates/cameo-pi-core` | cameo-pi `crates/core` | `d60d963` (0.23.0) | Its pin test now checks this file. |
| `apps/server` | cameo-pi `apps/server` | `d60d963` (0.23.0) | The server package retains its imported name; milestone 3 names its executable `cutcraft-web`. |
| `docs/reference/cameo-pi-spec.md` | cameo-pi `docs/spec.md` | `d60d963` | Reference for cutting, the library and backups. |

**Full origin commits:**
- **CutCraft 1.4.2:** `5462a356a41987e32a9ff4e4b1b17f386cf831da`
- **CutCraft 1.2.1** (cameo-pi's pin): `b6aad512f19217677c9675def794087e972fa89d`
- **cameo-pi 0.23.0:** `d60d963a24517df79ec968a8d04b3cdb42b0af31`

## What changed in the merge

- **One workspace, one version** (0.1.0). The crates keep their names for now.
- **The lock file** started from CutCraft's, so VectorCraft and its dependencies keep CutCraft's
  tested versions. Crates only cameo-pi used were added at cameo-pi's locked versions, the SSH and
  crypto chain for backups included.
- **Feature unification:** `serde_json`'s `float_roundtrip`, which cameo-pi enabled so a project
  read back plans exactly as it was planned, now applies to CutCraft's crates too.
  - **Checked:** CutCraft's corpus check on the developer's 142-design acceptance set gives
    identical results, geometry hashes included, with and without the feature.
  - **A separate finding:** 57 of those entries no longer match their recorded expectations in
    either build. They were recorded before Make cut-ready asked about very small and very thin
    pieces. Re-recording them is the developer's decision.
- **Pronouns:** "she" and "her" for the end user became "they" and "their" in comments and in the
  command descriptions AI tools read (64 lines), as this project's rules require. Nine test names and
  one manifest comment followed (Grok's re-review).
- **Dependency updates:** tokio 1.53.2 to 1.54.1 (an unsoundness advisory) and crypto-bigint 0.7.8
  to 0.7.10 (yanked). `cargo deny` (`deny.toml`) now guards sources, licences and advisories. The
  IJG licence (jpeg-encoder, through VectorCraft's PDF output) is allowed: it's permissive and
  asks for credit in the documentation.
- **The planner's identity** stays `1.2.1 (b6aad51)` (`crates/cameo-pi-core/src/validate.rs`):
  the planner crates' code is unchanged since that commit (only comments changed), so library
  revisions planned by cameo-pi still match. The test that reads this file only checks that the
  record agrees with the constant. It isn't a fingerprint of the algorithm: any later change to
  planning code must change the planner identity by hand.
- **The spike** (`spike/m0`) built from these imported crates until milestone 2 retired it: its
  server became `apps/server/src/design`, and its protocol check became `scripts/design-check.sh`.
  Its browser slice returns as the desktop layout in milestone 4. Git history keeps it.

## Not imported yet

| What | When |
|---|---|
| CutCraft's `crates/mcp`, `apps/cli` and the desktop app | Milestone 4 (the desktop UI, the test harness and the corpus runner), milestone 6 (MCP) |
| cameo-pi's `apps/probe` | With the hardware checks |
| cameo-pi's packaging (`packaging/`, the `.deb`, the apt repository, the SD image, the release workflow) | Milestone 8 |
| cameo-pi's `scripts/browser-check.*` and `scripts/backup-check.sh` | This milestone; they run against `apps/server` |

## External code

- **VectorCraft** (`storytold/vectorcraft`) at `fa4e427`, and the patched PDF crates from
  `storytold/craft-libs` at `ca50d63`. Both are public, and pinned to full commits.

## Desktop chrome (milestone 4)

`crates/ui/src/chrome.rs` carries the original vector toolbar icons and panel group headers
from CutCraft 1.0.5, commit `39654a9465ecdef7478703ee586276304dbc29c6`, under the same
MIT OR Apache-2.0 licence. It uses egui directly and has neutral comments. The browser
transport and desktop adapter are implemented here; no runtime dependency on that repository.

The choice labels and issue/info titles in `crates/ui/src/panels.rs` also derive from
the same reference app’s `choose.rs` and `prepare.rs`, with browser-safe inputs.
