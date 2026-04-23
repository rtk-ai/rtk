{
  description = "rtk - CLI proxy that reduces LLM token consumption by 60-90%";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        cargoToml = pkgs.lib.trivial.importTOML ./Cargo.toml;
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "rtk";
          version = cargoToml.package.version;

          src = ./.;

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            sqlite
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
          ];

          # rusqlite uses "bundled" feature — it compiles its own SQLite,
          # so we disable pkg-config detection to avoid conflicts.
          RUSQLITE_USE_PKG_CONFIG = "0";

          # test_tool_exists_finds_git asserts `git` is on PATH during tests.
          nativeCheckInputs = with pkgs; [ git ];

          # Tests need a writable HOME (for dirs::data_local_dir) and
          # RTK_DB_PATH (tracking tests create an SQLite database).
          preCheck = ''
            export HOME="$(mktemp -d)"
            export RTK_DB_PATH="$HOME/rtk-test.db"
          '';

          meta = with pkgs.lib; {
            description = "CLI proxy that reduces LLM token consumption by 60-90% on common dev commands";
            homepage = "https://github.com/rtk-ai/rtk";
            license = licenses.mit;
            maintainers = [ ];
            mainProgram = "rtk";
          };
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.default ];
          packages = with pkgs; [
            rust-analyzer
            clippy
            rustfmt
          ];
        };
      }
    ) // {
      # NixOS module — allows `programs.rtk.enable = true;` in NixOS configurations
      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.programs.rtk;
        in
        {
          options.programs.rtk = {
            enable = lib.mkEnableOption "rtk CLI token-reduction proxy";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              description = "The rtk package to use.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
          };
        };
    };
}
