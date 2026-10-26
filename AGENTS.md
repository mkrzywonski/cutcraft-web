# CutCraft-Web contributor instructions

- **New to this repo? Read [docs/spec.md](docs/spec.md).** It's a design spec; there is no code yet.
  Raise new questions with the developer rather than deciding them; the spec records the answers.
- CutCraft-Web is the one product. It replaces CutCraft and cameo-pi, whose repositories are
  private and will be deleted: their code is imported here (spec §5.2) and recorded in
  `docs/provenance.md`. Never depend on either old repository, and don't bring over their revision history.
  VectorCraft stays an external dependency pinned to a full commit hash.
- Coordinate writers: one agent edits a checkout at a time. Use a separate worktree for any
  explicitly requested parallel work.
- Clean room: never copy code from GPL/AGPL projects, including inkscape-silhouette, OctoPrint
  and OctoPi. Running GPL programs (Samba, for example) is fine; copying their code is not.
- Private design files, their names, extracted assets and anything generated from them stay
  local and are never committed, even in logs, docs, screenshots or test snapshots. Locate the
  archive through `CUTCRAFT_CORPUS_DIR`; outputs go in the gitignored `local/`. Tests use
  original authored fixtures. Never commit a cutter serial, Wi-Fi details, SSH keys, access keys,
  LAN addresses or personal names: write "the developer" and "the end user" (they/them).
- On NixOS use `nix-shell`/`nix develop` for this repo; don't install tools globally. System
  changes belong in the developer's NixOS configuration (`~/nix-config`).
- Rust: edition 2024, toolchain pinned as in CutCraft, `unsafe` forbidden, `Cargo.lock` kept,
  `--locked` everywhere. Before committing, run `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked` and `cargo build --workspace --locked`.
- **Physical motion needs the developer present and confirming.** Develop against the Simulated
  Cameo. Discovery never opens devices or sends commands.
- Never write cryptography: rustls with ring, rcgen, argon2, sha2, getrandom, as in cameo-pi.
- Every user-facing action is one validated action in the program, the same from both layouts
  and MCP, through CutCraft's command registry. Both layouts have the same capabilities. There is
  no separate state for any client (spec §5, §9).
- **Simple UX:** fewer concepts, sensible defaults, no extra prompts.
- Once the program exists, every commit that changes it raises the version (patch for fixes,
  minor for features, major only when the developer asks) and adds a `CHANGELOG.md` entry.
- When a requested task is finished, send the developer an ntfy notification.
