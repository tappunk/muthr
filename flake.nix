{
  description = "Zero-trust orchestration for autonomous AI agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [ "aarch64-darwin" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        version = cargoToml.package.version;
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
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

          buildInputs = with pkgs.darwin.apple_sdk.frameworks; [
            Security
            SystemConfiguration
            Metal
            Foundation
            Accelerate
          ];

          postInstall = ''
            installShellCompletion --cmd muthr \
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

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustc
            cargo
            clippy
            rustfmt
            rust-analyzer

            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
            darwin.apple_sdk.frameworks.Metal
            darwin.apple_sdk.frameworks.Foundation
            darwin.apple_sdk.frameworks.Accelerate
          ];

          shellHook = ''
            echo "muthr dev environment loaded (aarch64-darwin)"
            echo "Version: ${version}"
          '';
        };

        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/muthr";
        };
      });
}
