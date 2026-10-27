# Changelog

Each version of CutCraft-Web and what changed in it. Versions are `major.minor.patch`: patch for
fixes and small changes, minor for new features, major for big changes. Raise it with
`scripts/bump-version.sh patch|minor|major`.

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
