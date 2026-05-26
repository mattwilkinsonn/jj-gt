set shell := ["bash", "-c"]
set dotenv-load := false

default:
    @just --list

# Install supported hook runners + the lint binaries hk.pkl needs + the
# `gt` (Graphite) and `gh` (GitHub) CLIs jj-gt shells out to. macOS uses
# Homebrew. Linux uses `uv` for the Python backends, `npm` for
# markdownlint-cli2 and `@withgraphite/graphite-cli`, prebuilt tarballs
# for lefthook/actionlint/gh, and `cargo binstall` for hk.
install-deps:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
        Darwin)
            brew install pre-commit prek lefthook hk markdownlint-cli2 actionlint gh
            # Graphite CLI ships on npm only.
            if command -v npm >/dev/null 2>&1; then
                npm install -g @withgraphite/graphite-cli
            else
                echo "warn: npm not on PATH; install Node.js to get the gt CLI" >&2
            fi
            ;;
        Linux)
            # Respect XDG_BIN_HOME when set (CI sets it so all installed
            # tools live under one cacheable dir). Default to ~/.local/bin
            # for local dev installs.
            bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
            mkdir -p "$bin_dir"
            export PATH="$bin_dir:$PATH"

            uv tool install pre-commit
            uv tool install prek

            # Resolve latest stable tags via the github redirect (no
            # gh / jq dependency, no API auth needed). `/releases/latest`
            # 302-redirects to `/releases/tag/<latest>`; we read the
            # Location header.  This means we self-heal when upstream
            # rolls forward (no more pinned-version 404 surprise) at
            # the cost of one CI run potentially seeing a different
            # version than the last — acceptable for tools we just
            # need on PATH.
            resolve_latest() {
                local owner_repo="$1"
                curl -sI "https://github.com/${owner_repo}/releases/latest" \
                    | awk 'BEGIN{IGNORECASE=1} /^location:/ {sub(/\r$/,"",$2); n=split($2,p,"/"); print p[n]}'
            }

            lefthook_version=$(resolve_latest evilmartians/lefthook)
            lefthook_version_bare="${lefthook_version#v}"
            actionlint_version=$(resolve_latest rhysd/actionlint)
            actionlint_version_bare="${actionlint_version#v}"
            gh_version=$(resolve_latest cli/cli)
            gh_version_bare="${gh_version#v}"

            arch="$(uname -m)"
            case "$arch" in
                x86_64)  lefthook_arch=x86_64; actionlint_arch=amd64; gh_arch=amd64 ;;
                aarch64) lefthook_arch=arm64;  actionlint_arch=arm64; gh_arch=arm64 ;;
                *)
                    echo "unsupported Linux arch: $arch" >&2
                    exit 1
                    ;;
            esac
            curl -fsSL "https://github.com/evilmartians/lefthook/releases/download/${lefthook_version}/lefthook_${lefthook_version_bare}_Linux_${lefthook_arch}" \
                -o "$bin_dir/lefthook"
            chmod +x "$bin_dir/lefthook"

            curl -fsSL "https://github.com/rhysd/actionlint/releases/download/${actionlint_version}/actionlint_${actionlint_version_bare}_linux_${actionlint_arch}.tar.gz" \
                | tar -xz -C "$bin_dir" actionlint

            # pkl stays pinned because apple/pkl tags aren't `v`-
            # prefixed and the release artefact names also vary
            # (linux-amd64 vs linux-aarch64 without the dash patterns
            # the resolver helper assumes). Bump manually if needed.
            pkl_version=0.31.1
            case "$arch" in
                x86_64)  pkl_arch=amd64 ;;
                aarch64) pkl_arch=aarch64 ;;
            esac
            curl -fsSL "https://github.com/apple/pkl/releases/download/${pkl_version}/pkl-linux-${pkl_arch}" \
                -o "$bin_dir/pkl"
            chmod +x "$bin_dir/pkl"

            # gh ships a prebuilt tarball. Extract just the binary.
            curl -fsSL "https://github.com/cli/cli/releases/download/${gh_version}/gh_${gh_version_bare}_linux_${gh_arch}.tar.gz" \
                | tar -xz --strip-components=2 -C "$bin_dir" "gh_${gh_version_bare}_linux_${gh_arch}/bin/gh"

            if command -v npm >/dev/null 2>&1; then
                npm config set prefix "$(dirname "$bin_dir")"
                npm install -g markdownlint-cli2 @withgraphite/graphite-cli
            else
                echo "warn: npm not on PATH; install Node.js to get markdownlint-cli2 + gt" >&2
            fi

            if ! command -v cargo-binstall >/dev/null 2>&1; then
                cargo install cargo-binstall
            fi
            cargo binstall -y --install-path "$bin_dir" hk
            ;;
        *)
            echo "unsupported OS: $(uname -s)" >&2
            exit 1
            ;;
    esac

# Verify all four runners + the lint binaries hk.pkl needs are on PATH.
# `gt` is not checked because jj-gt tests don't actually invoke it (they
# pin the constructed argv shape via unit tests). Same for `gh`.
check-deps:
    #!/usr/bin/env bash
    set -euo pipefail
    missing=()
    for bin in pre-commit prek lefthook hk pkl markdownlint-cli2 actionlint; do
        if ! command -v "$bin" >/dev/null 2>&1; then
            missing+=("$bin")
        fi
    done
    if [ ${#missing[@]} -gt 0 ]; then
        echo "missing tools: ${missing[*]}" >&2
        echo "run \`just install-deps\` to install them" >&2
        exit 1
    fi
    echo "all tools installed"

build:
    cargo build --all-targets

# Run the full test suite. Requires `just install-deps` to have been run first.
test: check-deps
    cargo nextest run --no-fail-fast

# Run only unit / pure tests that don't need external binaries.
test-pure:
    cargo nextest run --no-fail-fast --lib

# Live-test recipes. `test-live-gt` only needs `gt` on PATH and runs
# entirely against tempdir fixtures (no network). `test-live-gh` and
# `test-live-submit` hit real GitHub; run `setup-live-test-fixture`
# once before using either, and export the env vars they document.

test-live-gt:
    cargo nextest run --no-fail-fast --test gt_live

# Live `gh pr list` test against the fixture repo. Requires:
#   * scripts/setup-live-test-fixture.sh to have run once.
#   * JJ_GT_LIVE_GH=1
#   * JJ_GT_LIVE_REPO=<owner>/jj-gt-live-tests
test-live-gh:
    JJ_GT_LIVE_GH=1 \
    JJ_GT_LIVE_REPO="${JJ_GT_LIVE_REPO:-mattwilkinsonn/jj-gt-live-tests}" \
        cargo nextest run --no-fail-fast --test gh_live

# Live end-to-end `gt submit` test against the fixture repo. Creates
# real PRs (cleaned up at the end). Requires:
#   * JJ_GT_LIVE_SUBMIT=1
#   * JJ_GT_LIVE_REPO=<owner>/jj-gt-live-tests
#   * JJ_GT_LIVE_REPO_URL=https://github.com/<owner>/jj-gt-live-tests.git
test-live-submit:
    JJ_GT_LIVE_SUBMIT=1 \
    JJ_GT_LIVE_REPO="${JJ_GT_LIVE_REPO:-mattwilkinsonn/jj-gt-live-tests}" \
    JJ_GT_LIVE_REPO_URL="${JJ_GT_LIVE_REPO_URL:-https://github.com/mattwilkinsonn/jj-gt-live-tests.git}" \
        cargo nextest run --no-fail-fast --test gt_submit_live

# One-time bootstrap of the GitHub fixture repo used by the live gh +
# submit tests. Idempotent.
setup-live-fixture OWNER="mattwilkinsonn":
    ./scripts/setup-live-test-fixture.sh {{ OWNER }}

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --all-targets -- -D warnings

# Pre-commit check: fmt + clippy + tests.
ci: fmt-check clippy test

# Install a debug build to ~/.cargo/bin. Codesigns on macOS so the binary
# can be re-run without confirmation. No --release.
install-debug:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --bin jj-gt
    dest="${CARGO_HOME:-$HOME/.cargo}/bin"
    mkdir -p "$dest"
    # On Linux, writing over an in-use executable fails with ETXTBSY
    # (text file busy). Unlink first so a running process keeps its
    # inode while we drop a fresh one at the path. macOS lets you
    # overwrite an active binary, so the unlink is a no-op there.
    rm -f "$dest/jj-gt"
    cp "target/debug/jj-gt" "$dest/jj-gt"
    if [[ "$(uname)" == "Darwin" ]]; then
        codesign -s - "$dest/jj-gt" 2>/dev/null && echo "Codesigned jj-gt" || true
    fi
    echo "Installed debug build (jj-gt) to $dest"

# Install a release build to ~/.cargo/bin. Codesigns on macOS.
install: build-release
    #!/usr/bin/env bash
    set -euo pipefail
    dest="${CARGO_HOME:-$HOME/.cargo}/bin"
    mkdir -p "$dest"
    rm -f "$dest/jj-gt"
    cp "target/release/jj-gt" "$dest/jj-gt"
    if [[ "$(uname)" == "Darwin" ]]; then
        codesign -s - "$dest/jj-gt" 2>/dev/null && echo "Codesigned jj-gt" || true
    fi
    echo "Installed release build (jj-gt) to $dest"

build-release:
    cargo build --release --bin jj-gt

# Cut a release. Bumps Cargo.toml, refreshes Cargo.lock, commits the
# bump on top of @, tags @- with the version, advances the local `main`
# bookmark to the release commit, and pushes both the commit and the
# tag to origin. Triggers the release.yml workflow on push.
#
# Usage: just release v0.1.0
release VERSION:
    #!/usr/bin/env bash
    set -euo pipefail

    version="{{ VERSION }}"
    if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9._-]+)?$ ]]; then
        echo "error: VERSION must look like v1.2.3 or v1.2.3-rc.1 (got: $version)" >&2
        exit 1
    fi
    bare="${version#v}"

    # Require a clean @ -- release commits should not include unrelated work.
    if [ -n "$(jj diff --summary --ignore-working-copy 2>/dev/null)" ]; then
        echo "error: working copy @ has uncommitted changes; finalize them first" >&2
        exit 1
    fi

    # Require `main` to be an ancestor of `@` so the release commit lands
    # on top of main. Otherwise advancing main to @- after the commit
    # would move it backwards or sideways onto an unrelated branch.
    if ! jj --ignore-working-copy log -r "main & ::@" -T 'change_id' --no-graph 2>/dev/null | grep -q .; then
        echo "error: @ is not a descendant of main (run \`jj rebase -d main\` first)" >&2
        exit 1
    fi

    if jj --ignore-working-copy tag list -T 'name ++ "\n"' 2>/dev/null | grep -qx "$version"; then
        echo "error: tag $version already exists" >&2
        exit 1
    fi

    if ! cargo set-version --help >/dev/null 2>&1; then
        echo "error: cargo-edit not installed (run: cargo install --locked cargo-edit)" >&2
        exit 1
    fi

    echo "Setting package version to $bare..."
    cargo set-version "$bare"
    echo

    echo "Updating Cargo.lock..."
    cargo update --workspace
    echo

    echo "Committing release bump as a new jj change on top of @..."
    jj commit -m "release: $version"
    echo

    echo "Tagging @- with $version..."
    jj tag set "$version" -r @-
    echo

    # Move the local `main` bookmark forward to the release commit so
    # `jj git push` pushes the right ref.
    echo "Advancing main to the release commit..."
    jj bookmark set main -r @-
    echo

    echo "Exporting refs to git..."
    jj --ignore-working-copy git export >/dev/null 2>&1 || true
    echo

    echo "Pushing main..."
    jj git push -b main
    echo

    echo "Pushing tag $version (triggers release.yml)..."
    jj-push-tags "$version"
    echo

    echo "Done. Watch the release workflow:"
    echo "  https://github.com/mattwilkinsonn/jj-gt/actions/workflows/release.yml"
