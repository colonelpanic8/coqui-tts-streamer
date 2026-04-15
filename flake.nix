{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    flake-utils,
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        overlays = [(import rust-overlay)];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        cargoManifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        packageVersion = cargoManifest.workspace.package.version;
        gitCommitHash =
          if self ? shortRev
          then self.shortRev
          else if self ? dirtyShortRev
          then self.dirtyShortRev
          else if self ? rev
          then builtins.substring 0 12 self.rev
          else if self ? dirtyRev
          then builtins.substring 0 12 self.dirtyRev
          else "unknown";

        cargoPackageFlags = [
          "--package"
          "coqui-tts-streamer"
        ];

        coquiTtsStreamer = pkgs.rustPlatform.buildRustPackage {
          pname = "coqui-tts-streamer";
          version = packageVersion;
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          cargoBuildFlags = cargoPackageFlags;
          cargoTestFlags = cargoPackageFlags;
          cargoInstallFlags = [
            "--path"
            "crates/app"
          ];

          nativeBuildInputs = with pkgs; [
            git
            makeWrapper
            pkg-config
          ];

          GIT_COMMIT_HASH = gitCommitHash;

          postFixup = ''
            wrapProgram "$out/bin/coqui-tts-streamer" \
              --prefix PATH : ${pkgs.lib.makeBinPath [pkgs.ffmpeg]}
          '';

          meta.mainProgram = "coqui-tts-streamer";
        };
      in {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            ffmpeg
            git
            just
            pkg-config
            (rust-bin.stable.latest.default.override {
              extensions = ["rust-src"];
            })
          ];

          RUST_BACKTRACE = 1;

          shellHook = ''
            echo "coqui-tts-streamer development environment"
            echo "Rust version: $(rustc --version)"
            echo "Cargo version: $(cargo --version)"
            echo "ffplay version: $(ffplay -version | head -n 1)"
          '';
        };

        packages.default = coquiTtsStreamer;
        packages.coqui-tts-streamer = coquiTtsStreamer;
      }
    );
}
