{
  description = "A kanban client for ticket trackers in the terminal: vim keys, a fuzzy picker, Jira and GitHub Projects";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      packages = forAll (pkgs: rec {
        default = kolum;
        kolum = pkgs.rustPlatform.buildRustPackage {
          pname = "kolum";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          src = self;
          # Every dependency is from crates.io: the lockfile is enough.
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "-p" "kolum" ];
          cargoTestFlags = [ "--workspace" ];

          meta = {
            description = "A kanban client for ticket trackers in the terminal";
            homepage = "https://github.com/ColinTenaguillo/kolum";
            license = pkgs.lib.licenses.gpl3Plus;
            mainProgram = "kolum";
            platforms = systems;
          };
        };
      });
    };
}
