{
  description = "RTK - high-performance CLI proxy that reduces LLM token consumption by 60-90%";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          rtk = pkgs.rustPlatform.buildRustPackage {
            pname = cargoToml.package.name;
            version = cargoToml.package.version;

            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            # Tests hit the filesystem and search PATH — incompatible with Nix sandbox.
            # The upstream CI already runs the full test suite.
            doCheck = false;

            nativeBuildInputs = with pkgs; [ pkg-config ];

            buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin (with pkgs.darwin.apple_sdk.frameworks; [
              Security
              SystemConfiguration
            ]);

            meta = with pkgs.lib; {
              description = cargoToml.package.description;
              homepage = cargoToml.package.homepage;
              license = licenses.asl20;
              maintainers = [ ];
              mainProgram = cargoToml.package.name;
              platforms = platforms.unix;
            };
          };
        in
        {
          default = rtk;
          inherit rtk;
        });

      devShells = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.rtk ];
            packages = with pkgs; [ rust-analyzer clippy rustfmt ];
          };
        });

      overlays.default = final: prev: {
        rtk = self.packages.${prev.system}.rtk;
      };
    };
}
