{
  description = "A zero-trust orchestrator that automates secure inference and isolated execution of local AI agents.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "aarch64-darwin";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      packages.aarch64-darwin.default = pkgs.rustPlatform.buildRustPackage {
        pname = "muthr";
        version = "0.1.42";
        src = ./.;

        cargoLock = {
          lockFile = ./Cargo.lock;
        };

        nativeBuildInputs = with pkgs; [ installShellFiles ];

        meta = with pkgs.lib; {
          description = "A zero-trust orchestrator that automates secure inference and isolated execution of local AI agents.";
          homepage = "https://github.com/tappunk/muthr";
          license = licenses.mit;
          maintainers = [ ];
          platforms = [ "aarch64-darwin" ];
          mainProgram = "muthr";
        };
      };

      devShells.aarch64-darwin.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          rustc
          cargo
          clippy
          rustfmt
          rust-analyzer
        ];

        shellHook = ''
          echo "muthr dev environment loaded (aarch64-darwin)"
        '';
      };

      apps.aarch64-darwin.default = {
        type = "app";
        program = "${self.packages.aarch64-darwin.default}/bin/muthr";
      };
    };
}