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

**Status:** the revised [specification](docs/spec.md) includes the CutCraft 1.0.5 parity
requirements. This is the independent M4 implementation branch, still under development:
[M4 progress and remaining gates](docs/m4-progress.md). The M3 service remains its base.

For development, enter `nix-shell`, run `scripts/build-desktop.sh`, then
`cargo build --workspace --locked`. The first shell entry builds the pinned WebAssembly
packaging tool; subsequent builds and browser tests use that tool without downloading it.
The server executable is `target/debug/cutcraft-web`. Use `--practice` for simulator
work, an isolated `--data` directory, and `--insecure-http` only for local checks.
[Update draining and simulator checks](ops/updates.md) describe the M3 operational contract.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
