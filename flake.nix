{
  description = "CutCraft-Web: CutCraft in the browser, self-hosted";

  inputs = {
    # The same nixpkgs as CutCraft.
    nixpkgs.url = "github:NixOS/nixpkgs/0d9e9b832d03ac387417e16ce1febf73b2e631e1";
    # Official prebuilt Rust, which includes the Pi's static target (nixpkgs' cross rustc
    # would have to be built from source).
    rust-overlay = {
      url = "github:oxalica/rust-overlay/ed3a19fd0439ed618ec5fe1e12f0ba69a8be38b5";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEach =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      # The development shell, as `nix-shell` gives without flakes.
      devShells = forEach (pkgs: {
        default = import ./shell.nix { inherit pkgs; };
      });
    };
}
