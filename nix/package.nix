{lib, rustPlatform}:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = "rtk";
  inherit (cargoToml.package) version;
  src = lib.cleanSource ./..;
  cargoLock.lockFile = ../Cargo.lock;
  doCheck = false; # tests require network
  meta = {
    description = "High-performance CLI proxy to minimize LLM token consumption";
    homepage = "https://github.com/rtk-ai/rtk";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
    mainProgram = "rtk";
  };
}
