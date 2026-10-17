{
  description = "Ultra-fast, native Rust autonomous AI terminal agent for DeepSeek, Gemini, Claude, OpenAI & Local LLMs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "corex";
          version = "0.7.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            openssl
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
          ];

          postInstall = ''
            ln -s $out/bin/cx $out/bin/corex 2>/dev/null || true
            ln -s $out/bin/cx $out/bin/cxs 2>/dev/null || true
          '';

          meta = with pkgs.lib; {
            description = "High-performance autonomous terminal agent for DeepSeek API & local LLMs";
            homepage = "https://github.com/sluisr/corex";
            license = licenses.agpl3Only;
            mainProgram = "cx";
          };
        };

        apps.default = flake-utils.lib.mkApp {
          drv = self.packages.${system}.default;
          name = "cx";
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            pkg-config
            openssl
          ];
        };
      }
    );
}
