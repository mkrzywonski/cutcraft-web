# CutCraft-Web

**CutCraft in the browser, self-hosted.** One small home server with the Silhouette Cameo plugged
in: open it in a browser on any computer, tablet or phone, prepare a design and cut it. Nothing to
install on the computers, no USB drivers, one place for every project.

It replaces two earlier projects by the same developer, and brings their code in:
- **CutCraft:** a desktop app that makes designs cut-ready and cuts them on an original Silhouette
  Cameo.
- **cameo-pi:** a network cut server with a library, phone-friendly pages, backups and MCP.

On a computer it looks and works like CutCraft. A phone gets its own thumb-friendly pages, with the
same capabilities.

**Status:** the [specification](docs/spec.md) is final (version 1.1). Milestone 0's working slice is
built ([report](docs/m0-report.md)), and milestone 1 brought the earlier projects' code in
([provenance](docs/provenance.md)). Authority/session fixes and the M3 cut-handoff and
isolation implementation are on the review branch; see [M3 evidence](docs/m3-progress.md).

For development, build with `nix-shell --run 'cargo build --workspace --locked'`.
The server executable is `target/debug/cutcraft-web`. Use `--practice` for simulator
work, an isolated `--data` directory, and `--insecure-http` only for local checks.
[Update draining and simulator checks](ops/updates.md) describe the M3 operational contract.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
