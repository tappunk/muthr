{
  description = "Zero-trust orchestration for autonomous AI agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "aarch64-darwin";
      pkgs = import nixpkgs { inherit system; };
      
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    in
    {
      packages.${system} = {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          version = cargoToml.package.version;

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = [ pkgs.installShellFiles ];

          buildInputs = with pkgs.darwin.apple_sdk.frameworks; [
            Security
            SystemConfiguration
          ];

          postInstall = ''
            installShellCompletion --cmd muthr \
              --bash <($out/bin/muthr completion bash) \
              --zsh <($out/bin/muthr completion zsh)
          '';

          meta = with pkgs.lib; {
            description = cargoToml.package.description;
            homepage = cargoToml.package.repository;
            license = licenses.mit;
            platforms = [ "aarch64-darwin" ];
          };
        };
      };

      devShells.${system} = {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
          ] ++ (with pkgs.darwin.apple_sdk.frameworks; [
            Security
            SystemConfiguration
          ]);
        };
      };
    };
}
