{
  inputs = {
    v_flakes.url = "github:valeratrades/v_flakes?ref=v1.6";
  };
  outputs = { self, v_flakes }:
    let
      inherit (v_flakes) flake-utils pre-commit-hooks rust-overlay;
    in
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import v_flakes.default_nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        # Canonical toolchain pinned in v_flakes — byte-identical across repos, so
        # the nix store dedups it and sccache cross-references compilations.
        rust = v_flakes.rs.default_nightly system;
        # Lean toolchain for the production image build: rustc + cargo + std only.
        rustBuild = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.minimal);
        pname = "tg_sync";
        cargoToml = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package;

        rs = v_flakes.rs { inherit pkgs rust; };
        github = v_flakes.github {
          inherit pkgs pname rs;
          enable = true;
          cache = { cachix = "ev-invest"; };
          lastSupportedVersion = "nightly-2026-05-12";
          containerRelease = { registry = "ghcr.io/ev-invest"; };
          gitignore.extra = ''
            ## Env
            .env
            .env.local
            ## LLMs
            AGENTS.md
            CLAUDE.md
            .claude/
          '';
        };
        combined = v_flakes.utils.combine { inherit rust; modules = [ rs github ]; };

        rustPlatform = pkgs.makeRustPlatform { cargo = rustBuild; rustc = rustBuild; };
        tgSyncBin = rustPlatform.buildRustPackage {
          pname = cargoToml.name;
          version = cargoToml.version;
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            # .cargo holds dev-only accelerators (sccache rustc-wrapper) the hermetic
            # sandbox lacks — let nix's vendor config drive the build instead.
            filter = path: _type: ! builtins.elem (baseNameOf path) [ "target" ".direnv" ".git" ".cargo" "result" ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          doCheck = false;
        };

        # No public surface: the contract's port only exists so k8s has something to
        # probe (the v_flakes standard generates httpGet liveness/readiness from it),
        # and gitops gives it no IngressRoute. The topic map, chat id and media cap
        # are authored in gitops' flake; only the bot token and the webhook URLs are
        # secret, and they arrive through the optional `kubernetes-tg-sync` envFrom.
        containerStd = v_flakes.container.implement {
          inherit pkgs pname;
          containers."" = {
            port = 55680;
            healthPath = "/health";
            criticality = "normal";
            entrypoint = [ "/bin/tg_sync" ];
            contents = [ tgSyncBin ];
            env = {
              BIND = "0.0.0.0:55680";
              APP_ENV = "production";
              RUST_LOG = "info";
            };
          };
        };

        pre-commit-check = pre-commit-hooks.lib.${system}.run {
          src = ./.;
          hooks = {
            treefmt = {
              enable = true;
              packageOverrides.treefmt = pkgs.treefmt;
              entry = pkgs.lib.mkForce "bash -c 'treefmt --no-cache \"$@\" && git add -u' --";
              require_serial = true;
            };
          };
        };
      in
      {
        packages = { default = tgSyncBin; tg_sync = tgSyncBin; } // containerStd.packages;
        containers = containerStd.containers;

        devShells.default = with pkgs; mkShell {
          shellHook = pre-commit-check.shellHook + combined.shellHook;
          packages = [ openssl pkg-config clang rust sccache mold treefmt nixpkgs-fmt ]
            ++ pre-commit-check.enabledPackages ++ combined.enabledPackages;
          env.RUST_BACKTRACE = 1;
          env.RUST_LIB_BACKTRACE = 0;
          env.DYLD_FALLBACK_LIBRARY_PATH = "${rust}/lib";
          env.RUSTC_WRAPPER = "sccache";
        };
      }
    );
}
