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
