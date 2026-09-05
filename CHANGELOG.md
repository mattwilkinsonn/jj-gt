# Changelog

All notable changes to jj-gt are tracked here.

## [Unreleased]

- Re-established as a standalone repository, extracted from the
  `mattwilkinsonn/zireael` monorepo at v0.3.11. Toolchain moved to devenv + its
  built-in `tasks` runner (dropping moon/proto), Rust pinned via rust-overlay, the
  `jj-hooks` dependency switched from a workspace path-dep to the published crates.io
  version, and shared dev tooling consumed from `mattwilkinsonn/dev-shared`.

## [0.3.11]

Baseline: the jj-gt state at monorepo extraction. Full pre-extraction history lives
in the zireael monorepo.
