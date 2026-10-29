# Match the exact wasm-bindgen version linked into the desktop wasm.
{ pkgs, rust }:
(pkgs.makeRustPlatform { cargo = rust; rustc = rust; }).buildRustPackage rec {
  pname = "wasm-bindgen-cli";
  version = "0.2.129";
  src = pkgs.fetchCrate {
    inherit pname version;
    sha256 = "sha256-pcecKQd7E8Opw6bkFoE569epUi7gh5qpQF1e5PJY6V8=";
  };
  cargoHash = "sha256-vmUrWVU7kPJJxO5qIVeAkwQyWDELO1Z4Z5gitz2kco8=";
  nativeBuildInputs = [ pkgs.pkg-config ];
  buildInputs = [ pkgs.openssl ];
  # Upstream integration tests need browser tooling; our browser gate exercises this CLI output.
  doCheck = false;
}
