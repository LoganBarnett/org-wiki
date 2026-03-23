{
  description = "Org-mode wiki with web editor and git-backed storage";
  inputs = {
    # LLM: Do NOT change this URL unless explicitly directed. This is the
    # correct format for nixpkgs stable (25.11 is correct, not nixos-25.11).
    nixpkgs.url = "github:NixOS/nixpkgs/25.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane.url = "github:ipetkov/crane";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    crane,
  } @ inputs: let
    forAllSystems = nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed;
    overlays = [
      (import rust-overlay)
    ];
    pkgsFor = system:
      import nixpkgs {
        inherit system;
        overlays = overlays;
      };

    # ============================================================================
    # WORKSPACE CRATES CONFIGURATION
    # ============================================================================
    # Define all workspace crates here. This makes it easy to:
    # - Generate packages
    # - Generate apps
    # - Generate overlays
    # - Keep package lists consistent across the flake
    #
    # When customizing this template for your project:
    # 1. Update the names below to match your project
    # 2. Add/remove crates as needed
    # 3. The package and app generation will automatically update
    # ============================================================================
    workspaceCrates = {

      # CRATE:web:begin
      # Web service
      web = {
        name = "org-wiki-web";
        binary = "org-wiki-web";
        description = "Org-mode wiki web server";
      };
      # CRATE:web:end

      # Note: The 'lib' crate is not included here as it doesn't produce a
      # binary.
    };

    # Development shell packages.
    devPackages = pkgs: let
      rust = pkgs.rust-bin.stable.latest.default.override {
        extensions = [
          # For rust-analyzer and others.  See
          # https://nixos.wiki/wiki/Rust#Shell.nix_example for some details.
          "rust-src"
          "rust-analyzer"
          "rustfmt"
        ];
      };
    in [
      rust
      pkgs.cargo-sweep
      pkgs.pkg-config
      pkgs.openssl
      pkgs.libgit2
      pkgs.pandoc
      pkgs.jq
      # Elm toolchain
      pkgs.elmPackages.elm
      pkgs.elmPackages.elm-format
      pkgs.elm2nix
      # Unified formatter
      pkgs.treefmt
      pkgs.alejandra
      pkgs.prettier
    ];
  in {
    devShells = forAllSystems (system: let
      pkgs = pkgsFor system;
    in {
      default = pkgs.mkShell {
        buildInputs = devPackages pkgs;
        shellHook = ''
          echo "Rust Template development environment"
          echo ""
          echo "Available Cargo packages (use 'cargo build -p <name>'):"
          cargo metadata --no-deps --format-version 1 2>/dev/null | \
            jq -r '.packages[].name' | \
            sort | \
            sed 's/^/  • /' || echo "  Run 'cargo init' to get started"

          echo ""
          echo "Elm frontend (frontend/):"
          echo "  Build:   cd frontend && elm make src/Main.elm --output public/elm.js"
          echo "  Format:  treefmt"
          echo "  After changing elm.json dependency versions, regenerate Nix files:"
          echo "    cd frontend"
          echo "    elm2nix convert 2>/dev/null > elm-srcs.nix"
          echo "    elm2nix snapshot"
          echo "    git add elm-srcs.nix registry.dat && git commit"

          # Symlink cargo-husky hooks into .git/hooks/ using paths relative
          # to .git/hooks/ so the repo stays valid after moves or copies.
          _git_root=$(git rev-parse --show-toplevel 2>/dev/null)
          if [ -n "$_git_root" ] && [ "$(pwd)" = "$_git_root" ] && [ -d ".cargo-husky/hooks" ]; then
            for _hook in .cargo-husky/hooks/*; do
              [ -x "$_hook" ] || continue
              _name=$(basename "$_hook")
              _dest="$_git_root/.git/hooks/$_name"
              _target=$(${pkgs.coreutils}/bin/realpath --relative-to="$_git_root/.git/hooks" "$(pwd)/$_hook")
              if [ ! -L "$_dest" ] || [ "$(readlink "$_dest")" != "$_target" ]; then
                ln -sf "$_target" "$_dest"
                echo "Installed git hook: $_name -> $_target"
              fi
            done
          fi
        '';
      };
    });

    # ============================================================================
    # PACKAGES
    # ============================================================================
    packages = forAllSystems (system: let
      pkgs = pkgsFor system;
      craneLib = (crane.mkLib pkgs).overrideToolchain (p: p.rust-bin.stable.latest.default);

      # Common build arguments shared by all crates
      commonArgs = {
        src = craneLib.cleanCargoSource ./.;
        # LLM: Do NOT add darwin.apple_sdk.frameworks here - they were removed
        # in nixpkgs 25.11+. Use libiconv for Darwin builds instead.
        buildInputs = with pkgs;
          [
            openssl
            libgit2
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin (with pkgs.darwin; [
            libiconv
          ]);
        nativeBuildInputs = with pkgs; [
          pkg-config
        ];
        # Run only unit tests (--lib --bins), skip integration tests in tests/ directories
        # Integration tests may require external services not available in Nix sandbox
        # Full test suite can be run locally with 'cargo test --all'
        cargoTestExtraArgs = "--lib --bins";
      };

      # Build individual crate packages from workspaceCrates.  When a
      # per-crate file exists under nix/packages/, it is used instead of
      # the generic crane build; this lets individual crates carry custom
      # build options without cluttering the top-level flake.
      cratePackages =
        pkgs.lib.mapAttrs (
          key: crate: let
            pkgFile = ./. + "/nix/packages/${key}.nix";
          in
            if builtins.pathExists pkgFile
            then import pkgFile {inherit craneLib commonArgs pkgs;}
            else
              craneLib.buildPackage (commonArgs
                // {
                  pname = crate.name;
                  cargoExtraArgs = "-p ${crate.name}";
                })
        )
        workspaceCrates;
    in
      cratePackages
      // {
        # Build all workspace binaries together.
        # Update pname to match your project name.
        default = craneLib.buildPackage (commonArgs // {pname = "org-wiki";});
      });

    # ============================================================================
    # APPS
    # ============================================================================
    apps = forAllSystems (system: let
      pkgs = pkgsFor system;
    in
      pkgs.lib.mapAttrs (key: crate: {
        type = "app";
        program = "${self.packages.${system}.${key}}/bin/${crate.binary}";
      })
      workspaceCrates);

    # ============================================================================
    # NIXOS MODULES
    # ============================================================================
    nixosModules = {
      web = import ./nix/modules/web.nix {inherit self;};
      default = self.nixosModules.web;
    };

    # ============================================================================
    # OVERLAYS
    # ============================================================================
    # Uncomment to expose your packages as an overlay
    # ============================================================================
    # overlays.default = final: prev:
    #   pkgs.lib.mapAttrs' (key: crate:
    #     pkgs.lib.nameValuePair crate.name self.packages.${final.stdenv.hostPlatform.system}.${key}
    #   ) workspaceCrates;
  };
}
