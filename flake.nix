{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable-small";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      overlays = [ rust-overlay.overlays.default ];

      pkgs = import nixpkgs {
        system = "aarch64-darwin";
        inherit overlays;
      };

      rustToolchainExtensions = [ "rust-src" "rust-analyzer" "clippy" ];
      rustToolchain = pkgs.rust-bin.stable.latest.default.override {
        targets = [ "aarch64-apple-darwin" ];
        extensions = rustToolchainExtensions;
      };

      commonPackages = p: with p; [ bacon ];

    in {
      devShells = {
        aarch64-darwin.default = pkgs.mkShell {
          packages = [ rustToolchain ] ++ (commonPackages pkgs);
          shellHook = ''
            echo "=== DEV SHELL (APPLE SILICON) ==="
          '';
        };
      };
    };
}
