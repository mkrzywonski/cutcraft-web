# Design audit validation — 2026-10-28

Reference: CutCraft 1.0.5, `39654a9465ecdef7478703ee586276304dbc29c6`.
This change edits specifications and preserves authored fixtures; it changes no application code.

- Built the reference CLI with `nix-shell --run 'cargo build -p cutcraft-cli --locked'`: passed.
- Ran initial `cutcraft-cli prepare` for all 21 issue fixtures with isolated configuration:
  all returned reports. Only fixtures 15 and 21 were immediately cut-ready. Outcomes are in
  [the manifest](fixtures.json); these are reference observations, not passing web tests.
- Compared the ledger's 162 unique full hashes against `git rev-list` at the reference:
  exact set and count, including five merges; row numbering is complete.
- Compared all 22 copied fixture files byte-for-byte against the reference and recomputed
  SHA-256 against the manifest: matched.
- Checked local Markdown links and heading anchors in the revised specification, companion,
  differences register, audit documents and example READMEs: resolved.
- Checked the detailed contract's 71 requirement IDs for uniqueness: no duplicates.
- Reviewed the documentation changes and fixture provenance for private paths, names,
  archive-derived artwork and credentials. Raw historical diffs and local reports are excluded.
- Documentation whitespace check against the branch base: passed. The full staged check reports
  trailing spaces in the authored PDF cross-reference records. Those bytes are intentional PDF
  formatting and are preserved exactly; the fixture is not rewritten to satisfy a text check.

No Rust application changes, so the workspace lint/test/build suite was not rerun for this
specification-only branch. No browser parity acceptance, Pi performance acceptance, physical
cutting or exhaustive historical builds were performed. The history review is a design and
regression audit; its method and limits are in [the coverage map](source-map.md).

The two port decisions in specification §23 were confirmed by the developer. The broader design
proposal still needs non-author review; this audit does not approve an implementation or a merge.
