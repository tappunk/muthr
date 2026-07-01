{
  description = "Zero-trust orchestrator for MLX inference, container-based sandboxes, and MCP services on Apple Silicon.";

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
        version = "0.1.46";
        src = ./.;

        cargoLock = {
          lockFile = ./Cargo.lock;
        };

        nativeBuildInputs = with pkgs; [ installShellFiles ];

        meta = with pkgs.lib; {
      description = "Zero-trust orchestrator for MLX inference, container-based sandboxes, and MCP services on Apple Silicon.";
          homepage = "https://github.com/tappunk/muthr";
          license = licenses.asl20;
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
