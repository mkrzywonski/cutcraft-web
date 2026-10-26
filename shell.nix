# Pinned development environment; enter with `nix-shell` (or `nix develop`).
# Without flakes, the inputs come from flake.lock, so both ways give the same toolchain.
{
  pkgs ?
    let
      lock = builtins.fromJSON (builtins.readFile ./flake.lock);
      fetch =
        name:
        let
          node = lock.nodes.${name}.locked;
        in
        builtins.fetchTarball {
          url = "https://github.com/${node.owner}/${node.repo}/archive/${node.rev}.tar.gz";
          sha256 = node.narHash;
        };
    in
    import (fetch "nixpkgs") { overlays = [ (import (fetch "rust-overlay")) ]; },
}:
let
  # Rust 1.95.0 with clippy, rustfmt and the aarch64 musl target, from rust-toolchain.toml.
  rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
in
pkgs.mkShell {
  nativeBuildInputs = [
    rust
    pkgs.git
    # The C compiler for the Pi build (scripts/zig-cc-aarch64-musl).
    pkgs.zig
    # The Debian package (scripts/build-deb.sh, scripts/apt-repo.sh).
    pkgs.dpkg
  ];
}
