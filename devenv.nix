{ pkgs, inputs, ... }:
# jj-gt dev shell. The shared toolchain (rust-overlay pin, linters, cargo-nextest, jj, and
# the ci:markdownlint/actionlint/nixfmt/deadnix lint task set) comes from the dev-shared
# module imported in devenv.yaml. This file adds only what is jj-gt-specific: the gh CLI its
# live tests drive, the hk pre-push runner, the crate's own ci:* tasks (default test excludes
# the live suite), the opt-in live-test task, and the hk pre-push gate install.
{
  # Pin jujutsu to 0.42.0. jj-gt's stack-reconciliation logic targets jj 0.42 semantics;
  # jj 0.44 (what rolling nixpkgs / the dev-shared module now provides) changed `jj git fetch`
  # to eagerly auto-rebase children onto rewritten parents, which breaks the orphan re-anchor
  # path. This overlay shadows the shared `jujutsu` with the pinned-rev build (devenv.yaml
  # nixpkgs-jj042), so both this file and the dev-shared module resolve jj 0.42.
  overlays = [
    (_final: prev: {
      jujutsu = (import inputs.nixpkgs-jj042 { system = prev.stdenv.system; }).jujutsu;
    })
  ];
  packages = with pkgs; [
    # jj-gt's live integration tests (gh_live / gt_submit_live) shell out to the gh CLI.
    gh
    # hk runs the pre-push gate (from its flake input — not in nixpkgs). Not a test backend here.
    inputs.hk.packages.${pkgs.stdenv.system}.hk
  ];
  # Crate checks. Named ci:* so `devenv tasks run ci` (namespace-prefix selector) runs them
  # with the shared ci:markdownlint/actionlint/nixfmt/deadnix. NEVER a bare `ci` task.
  # ci:test excludes the live (network) suite; those run only in the fork-gated CI live-test job.
  tasks = {
    "ci:fmt".exec = "cargo fmt --check";
    "ci:clippy".exec = "cargo clippy --all-targets -- -D warnings";
    "ci:test".exec =
      "cargo nextest run --no-fail-fast --no-tests=warn -E 'not (test(gh_live) | test(gt_submit_live))'";
    # Live suite — opt-in, never in the ci: gate. Needs JJ_GT_LIVE_* env + gh auth + a fixture
    # repo. Namespaced (live:test) because devenv rejects a bare task name — same rule as ci:*.
    "live:test" = {
      exec = "cargo nextest run --no-fail-fast -E 'test(gh_live) | test(gt_submit_live)'";
      env = {
        JJ_GT_LIVE_GH = "1";
        JJ_GT_LIVE_SUBMIT = "1";
      };
    };
  };

  enterShell = ''
    # Install the pre-push git hook (a thin shell over `devenv tasks run ci`). Idempotent.
    if command -v hk >/dev/null 2>&1; then
      hk install >/dev/null 2>&1 || echo "devenv: hk install failed; run 'hk install' to enable the pre-push gate"
    fi
  '';
}
