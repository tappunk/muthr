{
  description = "Zero-trust orchestration for autonomous AI agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "aarch64-darwin";
      pkgs = nixpkgs.legacyPackages.${system};

      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = cargoToml.package.version;
    in
    {
      packages.aarch64-darwin.default = pkgs.rustPlatform.buildRustPackage {
        pname = "muthr";
        inherit version;
        src = ./.;

        cargoLock = {
          lockFile = ./Cargo.lock;
        };

        nativeBuildInputs = with pkgs; [
          installShellFiles
          pkg-config
        ];

        buildInputs = with pkgs.darwin; [
          Security
          SystemConfiguration
          Metal
          Foundation
          Accelerate
        ];

        postInstall = ''
          installShellFiles --cmd muthr \
            --bash <($out/bin/muthr completion bash) \
            --zsh <($out/bin/muthr completion zsh)
        '';

        meta = with pkgs.lib; {
          description = "Zero-trust orchestration for autonomous AI agents";
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

          darwin.Security
          darwin.SystemConfiguration
          darwin.Metal
          darwin.Foundation
          darwin.Accelerate
        ];

        shellHook = ''
          echo "muthr dev environment loaded (aarch64-darwin)"
          echo "Version: ${version}"
        '';
      };

      apps.aarch64-darwin.default = {
        type = "app";
        program = "${self.packages.aarch64-darwin.default}/bin/muthr";
      };
    };
}
