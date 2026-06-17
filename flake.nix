{
  description = "Zero-trust orchestration for autonomous AI agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "aarch64-darwin";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "muthr";
        version = "0.1.7";
        src = ./.;

        cargoLock = {
          lockFile = ./Cargo.lock;
        };

        nativeBuildInputs = [ pkgs.installShellFiles ];

        # Force usage of modern SDK frameworks provided by the system
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
          platforms = [ "aarch64-darwin" ];
        };
      };
    };
}
