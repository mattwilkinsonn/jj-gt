# Changelog

All notable changes to jj-gt are tracked here.

## [Unreleased]

## [0.3.12]

Distribution-only release: re-anchors crates.io `repository`/binstall metadata
at the standalone repository and is the first release through the consolidated
`mattwilkinsonn/tap`. No functional code change.

- Re-established as a standalone repository, extracted from the
  `mattwilkinsonn/zireael` monorepo at v0.3.11. Toolchain moved to devenv + its
  built-in `tasks` runner (dropping moon/proto), Rust pinned via rust-overlay, the
  `jj-hooks` dependency switched from a workspace path-dep to the published crates.io
  version, and shared dev tooling consumed from `mattwilkinsonn/dev-shared`.

## [0.3.11]

Baseline: the jj-gt state at monorepo extraction. Full pre-extraction history lives
in the zireael monorepo.
