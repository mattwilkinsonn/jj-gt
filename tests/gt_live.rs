//! Live integration tests against the `gt` (Graphite) CLI.
//!
//! These tests build a colocated jj/git repo in a temp dir, run
//! `gt init --trunk main --no-interactive` to write the Graphite
//! sidecar config, then exercise `jj_gt::gt::track` and assert that
//! the resulting `refs/branch-metadata/<branch>` ref's blob contains
//! the expected `parentBranchName`.
//!
//! No network calls — `gt track` is purely local. Tests skip silently
//! when `gt` (or `jj`) isn't on PATH so they don't fail in
//! environments that haven't installed them yet.

use std::path::Path;
use std::process::Command;

use jj_gt::gt;
use jj_gt::jj::JjCli;
use jj_gt::stack::{derive_parents, sort_for_tracking};

fn binary_available(name: &str) -> bool {
    Command::new(name)
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

fn gt_cli(cwd: &Path, args: &[&str]) {
    let out = Command::new("gt")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "gt {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

fn git_capture(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Build a fixture jj+git repo with a 3-bookmark linear stack on top
/// of `main`. Returns the TempDir guarding the cleanup.
///
/// Layout after this returns:
///
/// ```text
///   * top    bookmark
///   * mid    bookmark
///   * bottom bookmark
///   * main   bookmark (trunk)
///   * root
/// ```
fn build_three_stack_fixture() -> tempfile::TempDir {
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
    jj(tmp.path(), &["describe", "-m", "root"]);
    jj(tmp.path(), &["bookmark", "create", "main", "-r", "@"]);
    jj(tmp.path(), &["new", "-m", "bottom change"]);
    jj(tmp.path(), &["bookmark", "create", "bottom", "-r", "@"]);
    jj(tmp.path(), &["new", "-m", "mid change"]);
    jj(tmp.path(), &["bookmark", "create", "mid", "-r", "@"]);
    jj(tmp.path(), &["new", "-m", "top change"]);
    jj(tmp.path(), &["bookmark", "create", "top", "-r", "@"]);
    // jj `git init --colocate` keeps the git refs in sync on most ops,
    // but a final `git export` makes the contract explicit.
    jj(tmp.path(), &["git", "export"]);
    tmp
}

fn gt_init(tmp: &Path) {
    gt_cli(tmp, &["init", "--trunk", "main", "--no-interactive"]);
}

/// Read the parentBranchName field from
/// `refs/branch-metadata/<branch>`. Returns None if the ref doesn't
/// exist; panics on a malformed blob (since that signals gt itself
/// changed shape and we want to catch it loudly).
fn parent_in_metadata(workspace_root: &Path, branch: &str) -> Option<String> {
    let ref_name = format!("refs/branch-metadata/{branch}");
    let out = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &ref_name])
        .current_dir(workspace_root)
        .output()
        .unwrap();
    if !out.status.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let blob = git_capture(workspace_root, &["cat-file", "-p", &oid]);
    // Tiny JSON probe — avoid pulling serde_json into the test deps;
    // the field shape is stable enough to substring-match for the
    // smoke. Format: `"parentBranchName": "<name>"`.
    let key = r#""parentBranchName": ""#;
    let start = blob.find(key)? + key.len();
    let rest = &blob[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

#[test]
fn track_writes_metadata_ref_with_expected_parent() {
    if !binary_available("jj") || !binary_available("gt") {
        eprintln!("skipping: jj or gt not on PATH");
        return;
    }
    let tmp = build_three_stack_fixture();
    gt_init(tmp.path());

    gt::track(tmp.path(), "bottom", "main").unwrap();
    gt::track(tmp.path(), "mid", "bottom").unwrap();
    gt::track(tmp.path(), "top", "mid").unwrap();

    assert_eq!(
        parent_in_metadata(tmp.path(), "bottom").as_deref(),
        Some("main")
    );
    assert_eq!(
        parent_in_metadata(tmp.path(), "mid").as_deref(),
        Some("bottom")
    );
    assert_eq!(
        parent_in_metadata(tmp.path(), "top").as_deref(),
        Some("mid")
    );
}

#[test]
fn track_overwrites_existing_parent_on_re_invoke() {
    if !binary_available("jj") || !binary_available("gt") {
        eprintln!("skipping: jj or gt not on PATH");
        return;
    }
    let tmp = build_three_stack_fixture();
    gt_init(tmp.path());

    // Track with parent=main first.
    gt::track(tmp.path(), "bottom", "main").unwrap();
    assert_eq!(
        parent_in_metadata(tmp.path(), "bottom").as_deref(),
        Some("main")
    );

    // Now stand up a sibling trunk-adjacent branch and retrack
    // bottom onto it to verify the metadata gets rewritten.
    jj(
        tmp.path(),
        &["bookmark", "create", "alt_trunk", "-r", "main"],
    );
    jj(tmp.path(), &["git", "export"]);
    gt::track(tmp.path(), "alt_trunk", "main").unwrap();
    gt::track(tmp.path(), "bottom", "alt_trunk").unwrap();
    assert_eq!(
        parent_in_metadata(tmp.path(), "bottom").as_deref(),
        Some("alt_trunk"),
    );
}

#[test]
fn track_rejects_child_before_parent() {
    // gt 1.7.x errors with "Cannot perform this operation on untracked
    // branch X" if you try to track a child before its parent. We want
    // jj-gt to topo-sort and avoid this; the test pins the gt
    // behaviour we're working around.
    if !binary_available("jj") || !binary_available("gt") {
        eprintln!("skipping: jj or gt not on PATH");
        return;
    }
    let tmp = build_three_stack_fixture();
    gt_init(tmp.path());

    let err = gt::track(tmp.path(), "top", "mid");
    assert!(
        err.is_err(),
        "expected gt to reject tracking `top` while `mid` is untracked"
    );
}

#[test]
fn submit_path_orders_track_calls_bottom_to_top() {
    // End-to-end: derive_parents → sort_for_tracking → gt::track in
    // the same order jj-gt submit does. Passing an inverted user
    // order (-b top -b bottom -b mid) should still produce a
    // correctly tracked stack because of the topo sort.
    if !binary_available("jj") || !binary_available("gt") {
        eprintln!("skipping: jj or gt not on PATH");
        return;
    }
    let tmp = build_three_stack_fixture();
    gt_init(tmp.path());

    let jj_cli = JjCli::new(tmp.path().to_path_buf());
    let user_order = vec!["top".to_owned(), "bottom".to_owned(), "mid".to_owned()];
    let stacked = derive_parents(&jj_cli, &user_order, "main").unwrap();
    let sorted = sort_for_tracking(&stacked);

    for sb in &sorted {
        let parent = sb.parent.as_branch_name("main");
        gt::track(tmp.path(), &sb.name, parent).unwrap();
    }

    assert_eq!(
        parent_in_metadata(tmp.path(), "bottom").as_deref(),
        Some("main")
    );
    assert_eq!(
        parent_in_metadata(tmp.path(), "mid").as_deref(),
        Some("bottom")
    );
    assert_eq!(
        parent_in_metadata(tmp.path(), "top").as_deref(),
        Some("mid")
    );
}

#[test]
fn read_repo_config_trunk_round_trips_through_gt_init() {
    if !binary_available("jj") || !binary_available("gt") {
        eprintln!("skipping: jj or gt not on PATH");
        return;
    }
    let tmp = build_three_stack_fixture();
    gt_init(tmp.path());

    let trunk = gt::read_repo_config_trunk(tmp.path()).unwrap();
    assert_eq!(trunk.as_deref(), Some("main"));
}
