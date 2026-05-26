//! End-to-end test for stack parent derivation against a real `jj`
//! repo. Builds a three-bookmark linear stack and asserts that
//! `derive_parents` walks the revset graph correctly.
//!
//! Skipped silently when `jj` isn't on PATH so the test can live in
//! the default `cargo test` set without forcing a hard dep on jj in
//! CI matrices that haven't installed it yet.

use std::path::Path;
use std::process::Command;

use jj_gt::jj::JjCli;
use jj_gt::stack::{BookmarkOrTrunk, derive_parents, find_tip};

fn jj_available() -> bool {
    Command::new("jj")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn jj(cwd: &Path, args: &[&str]) {
    let out = Command::new("jj")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "jj {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

fn jj_capture(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("jj")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "jj {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Build a fixture jj repo with this shape:
///
/// ```text
///   * top    (top bookmark)
///   * mid    (mid bookmark)
///   * bottom (bottom bookmark)
///   * main   (trunk)
///   * root
/// ```
fn build_linear_stack_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    jj(tmp.path(), &["git", "init", "--colocate"]);
    jj(
        tmp.path(),
        &["config", "set", "--repo", "user.email", "test@example.com"],
    );
    jj(
        tmp.path(),
        &["config", "set", "--repo", "user.name", "Tester"],
    );

    // Commit on the root change so `main` has somewhere real to point.
    jj(tmp.path(), &["describe", "-m", "root commit"]);
    jj(tmp.path(), &["bookmark", "create", "main", "-r", "@"]);

    // Bottom.
    jj(tmp.path(), &["new", "-m", "bottom change"]);
    jj(tmp.path(), &["bookmark", "create", "bottom", "-r", "@"]);

    // Mid.
    jj(tmp.path(), &["new", "-m", "mid change"]);
    jj(tmp.path(), &["bookmark", "create", "mid", "-r", "@"]);

    // Top.
    jj(tmp.path(), &["new", "-m", "top change"]);
    jj(tmp.path(), &["bookmark", "create", "top", "-r", "@"]);

    tmp
}

#[test]
fn derive_parents_linear_three_stack() {
    if !jj_available() {
        eprintln!("skipping: jj not on PATH");
        return;
    }
    let tmp = build_linear_stack_fixture();
    let jj_cli = JjCli::new(tmp.path().to_path_buf());

    let bookmarks: Vec<String> = vec!["bottom".into(), "mid".into(), "top".into()];
    let stacked = derive_parents(&jj_cli, &bookmarks, "main").unwrap();

    let by_name: std::collections::HashMap<String, BookmarkOrTrunk> = stacked
        .iter()
        .map(|s| (s.name.clone(), s.parent.clone()))
        .collect();

    assert_eq!(by_name["bottom"], BookmarkOrTrunk::Trunk);
    assert_eq!(by_name["mid"], BookmarkOrTrunk::Bookmark("bottom".into()));
    assert_eq!(by_name["top"], BookmarkOrTrunk::Bookmark("mid".into()));

    let tip = find_tip(&stacked).unwrap();
    assert_eq!(tip, "top");
}

#[test]
fn bookmark_on_trunk_resolves_to_trunk_parent() {
    if !jj_available() {
        eprintln!("skipping: jj not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    jj(tmp.path(), &["git", "init", "--colocate"]);
    jj(
        tmp.path(),
        &["config", "set", "--repo", "user.email", "test@example.com"],
    );
    jj(
        tmp.path(),
        &["config", "set", "--repo", "user.name", "Tester"],
    );
    jj(tmp.path(), &["describe", "-m", "only commit"]);
    jj(tmp.path(), &["bookmark", "create", "main", "-r", "@"]);
    jj(tmp.path(), &["bookmark", "create", "solo", "-r", "@"]);

    let jj_cli = JjCli::new(tmp.path().to_path_buf());
    let stacked = derive_parents(&jj_cli, &["solo".into()], "main").unwrap();

    assert_eq!(stacked.len(), 1);
    assert_eq!(stacked[0].name, "solo");
    // `solo` and `main` point at the same commit. jj-gt treats main
    // as a special trunk name regardless of co-location with another
    // bookmark, so parent should resolve to Trunk.
    //
    // Note: the revset `heads(::solo & bookmarks() ~ solo ~ ::main)`
    // excludes everything reachable from main, so the result is the
    // empty set → Trunk parent.
    assert_eq!(stacked[0].parent, BookmarkOrTrunk::Trunk);
}

#[test]
fn bookmarks_in_revset_resolves_names() {
    if !jj_available() {
        eprintln!("skipping: jj not on PATH");
        return;
    }
    let tmp = build_linear_stack_fixture();
    let jj_cli = JjCli::new(tmp.path().to_path_buf());

    // bookmarks() & ::@ should include all three stack bookmarks.
    let names = jj_gt::jj::bookmarks_in_revset(&jj_cli, "bookmarks() & ::@").unwrap();
    let set: std::collections::HashSet<String> = names.into_iter().collect();
    for expected in ["bottom", "mid", "top", "main"] {
        assert!(set.contains(expected), "missing `{expected}` in {set:?}");
    }
}

#[test]
fn current_change_id_round_trips() {
    if !jj_available() {
        eprintln!("skipping: jj not on PATH");
        return;
    }
    let tmp = build_linear_stack_fixture();
    let jj_cli = JjCli::new(tmp.path().to_path_buf());

    let id = jj_gt::jj::current_change_id(&jj_cli).unwrap();
    assert!(!id.is_empty(), "expected a non-empty change id");

    // Compare against `jj log -r @` to sanity-check we're reading the
    // same thing.
    let direct = jj_capture(
        tmp.path(),
        &["log", "-r", "@", "--no-graph", "-T", "change_id"],
    );
    assert_eq!(id.trim(), direct.trim());
}

#[test]
fn list_local_bookmarks_returns_name_and_short_commit_id() {
    // Regression test: jj 0.40+ rejected our previous template
    // `name ++ " " ++ commit_id.short(12) ++ "\n"` because the
    // bookmark template scope has no top-level `commit_id` keyword.
    // The fix in src/jj.rs uses `self.normal_target().commit_id()`
    // under an if-guard; this test pins that the template stays
    // valid against whatever jj version we're running on.
    if !jj_available() {
        eprintln!("skipping: jj not on PATH");
        return;
    }
    let tmp = build_linear_stack_fixture();
    let jj_cli = JjCli::new(tmp.path().to_path_buf());

    let bookmarks = jj_gt::jj::list_local_bookmarks(&jj_cli).unwrap();
    let by_name: std::collections::HashMap<String, String> = bookmarks
        .into_iter()
        .map(|b| (b.name, b.commit_id))
        .collect();
    for name in ["bottom", "mid", "top", "main"] {
        let commit = by_name
            .get(name)
            .unwrap_or_else(|| panic!("missing bookmark `{name}` in {by_name:?}"));
        assert_eq!(
            commit.len(),
            12,
            "expected 12-char short id, got `{commit}`"
        );
        assert!(
            commit.chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex short id, got `{commit}`",
        );
    }
}

#[test]
fn orphan_rebase_moves_full_multi_commit_range() {
    // Regression test for the sea-501 multi-commit-bookmark
    // conflict bug: when a bookmark holds 2+ commits (the user did
    // `jj new -m A; jj new -m B; jj bookmark create bk`), the
    // naive `jj rebase -s <bookmark> -d trunk` only moves the tip
    // commit (the one the bookmark name resolves to). The earlier
    // commits get stranded on the old parent's branch, and any
    // file they created looks like it "appeared from nowhere" on
    // the rebased tip → 2-sided file-creation conflict.
    //
    // Fixture topology, before the rebase:
    //
    //   * upper_b   <- `upper` bookmark
    //   * upper_a
    //   * bottom_b  <- `bottom` bookmark (the "merged" parent)
    //   * bottom_a
    //   * main
    //
    // We simulate gt sync having deleted `bottom` by passing the
    // pre-deletion commit id to build_orphan_rebase_revset. After
    // the rebase, `upper`'s ancestry chain should be
    // `upper_b -> upper_a -> main` (i.e. both upper_a + upper_b
    // came across), with no stranded bottom_* commits between them.
    if !jj_available() {
        eprintln!("skipping: jj not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    jj(cwd, &["git", "init", "--colocate"]);
    jj(
        cwd,
        &["config", "set", "--repo", "user.email", "test@example.com"],
    );
    jj(cwd, &["config", "set", "--repo", "user.name", "Tester"]);
    jj(cwd, &["describe", "-m", "root"]);
    jj(cwd, &["bookmark", "create", "main", "-r", "@"]);

    // Two-commit "bottom" stack entry, holding its own files.
    jj(cwd, &["new", "-m", "bottom commit A: create file"]);
    std::fs::write(cwd.join("bottom.txt"), "from bottom A\n").unwrap();
    jj(cwd, &["new", "-m", "bottom commit B: extend file"]);
    std::fs::write(cwd.join("bottom.txt"), "from bottom A\nfrom bottom B\n").unwrap();
    jj(cwd, &["bookmark", "create", "bottom", "-r", "@"]);

    // Capture bottom's pre-deletion tip commit id — this is what
    // run_fetch reads out of bookmarks_before_sync.
    let bottom_commit = jj_capture(
        cwd,
        &[
            "log",
            "-r",
            "bottom",
            "--no-graph",
            "-T",
            "commit_id",
            "--limit",
            "1",
        ],
    )
    .trim()
    .to_owned();

    // Two-commit "upper" stack entry. Upper's commits modify upper's
    // OWN file (not bottom's). The point of the test is to verify
    // both upper_a and upper_b come across in the rebase; the
    // file-identity of bottom's content (which a real merge would
    // carry into main via squash) isn't what we're exercising.
    jj(cwd, &["new", "-m", "upper commit A: create upper file"]);
    std::fs::write(cwd.join("upper.txt"), "from upper A\n").unwrap();
    jj(cwd, &["new", "-m", "upper commit B: extend upper file"]);
    std::fs::write(cwd.join("upper.txt"), "from upper A\nfrom upper B\n").unwrap();
    jj(cwd, &["bookmark", "create", "upper", "-r", "@"]);
    jj(cwd, &["git", "export"]);

    // Run the rebase using the same revset jj-gt would build.
    let jj_cli = JjCli::new(cwd.to_path_buf());
    let revset = jj_gt::cleanup::build_orphan_rebase_revset(&bottom_commit, "upper");
    let outcome = jj_gt::jj::rebase(&jj_cli, &revset, "main").unwrap();

    assert!(
        matches!(outcome, jj_gt::jj::RebaseOutcome::Clean),
        "expected clean rebase, got {outcome:?}",
    );

    // Verify upper's ancestry: upper -> upper_a -> main, with no
    // bottom commits in between. The clean way to check: log
    // upper's ancestors that aren't reachable from main.
    let ancestry = jj_capture(
        cwd,
        &[
            "log",
            "-r",
            "main..upper",
            "--no-graph",
            "-T",
            r#"description.first_line() ++ "\n""#,
        ],
    );
    let lines: Vec<&str> = ancestry.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines.len(),
        2,
        "expected 2 commits in upper's ancestry above main, got: {ancestry}",
    );
    assert!(
        lines.iter().any(|l| l.contains("upper commit B")),
        "missing upper B commit: {ancestry}",
    );
    assert!(
        lines.iter().any(|l| l.contains("upper commit A")),
        "missing upper A commit — the multi-commit-range fix didn't move it: {ancestry}",
    );
    // And critically, NO bottom commits should be there.
    assert!(
        !lines.iter().any(|l| l.contains("bottom commit")),
        "bottom commits shouldn't be in upper's post-rebase ancestry: {ancestry}",
    );
}
