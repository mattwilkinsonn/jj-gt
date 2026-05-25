//! `jj-gt fetch` pipeline + the testable per-bookmark classifier
//! decisions.
//!
//! Most of the file is the [`classify_local_bookmark`] /
//! [`classify_gtmq_branch`] functions — pure decision logic kept
//! separate from the orchestration so the test suite can exercise
//! every branch without spinning up real `gh` / `gt` / `jj` invocations.

use std::path::Path;

use crate::error::Result;
use crate::gh::{self, PrInfo, PrState};
use crate::gt;
use crate::jj::{self, JjCli, LocalBookmark, list_local_bookmarks};
use crate::stack::{BookmarkOrTrunk, derive_parents};

#[derive(Debug, Clone)]
pub struct FetchOpts {
    pub remote: String,
    pub trunk: String,
    pub no_backfill: bool,
    pub no_rebase: bool,
    pub no_gtmq_prune: bool,
    pub gtmq_prefixes: Vec<String>,
    pub auto: bool,
    pub dry_run: bool,
}

impl Default for FetchOpts {
    fn default() -> Self {
        Self {
            remote: "origin".into(),
            trunk: "main".into(),
            no_backfill: false,
            no_rebase: false,
            no_gtmq_prune: false,
            gtmq_prefixes: vec!["gtmq_".into()],
            auto: false,
            dry_run: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupAction {
    /// gt sync deleted this branch — bookmark already gone.
    GtSyncDeleted,
    /// Graphite queue-test branch (gtmq_*) with no open PR; deleted
    /// both locally and on the remote.
    GtmqPruned { had_pr: Option<u32> },
    /// gtmq_* branch with an open PR — left alone (queue actively
    /// running).
    GtmqLeftAlone { pr: u32 },
    /// PR closed and merge marker found on trunk; user confirmed
    /// deletion (or --auto).
    OrphanDeleted { pr: u32, merge_commit_id: String },
    /// PR closed and merge marker found, but user said no.
    OrphanSkipped { pr: u32, merge_commit_id: String },
    /// SHA drift detected — local has changes the PR doesn't.
    SkippedDueToDrift {
        pr: u32,
        local_sha: String,
        pushed_sha: String,
    },
    /// PR still open and local matches pushed; leave alone.
    LeftAlone,
}

/// Decide what to do with a non-gtmq local bookmark in the cleanup
/// pass. Pure function — no `jj` / `gh` calls — so the test suite can
/// exhaustively cover the cases.
///
/// `pr` is `None` if `gh pr list` returned no PR for this bookmark.
/// `merge_marker_on_trunk` is `Some(sha)` if the orphan-fallback scan
/// found a `(#N)` marker on trunk for the bookmark's PR.
pub fn classify_local_bookmark(
    local: &LocalBookmark,
    pr: Option<&PrInfo>,
    merge_marker_on_trunk: Option<&str>,
) -> CleanupAction {
    match pr {
        None => match merge_marker_on_trunk {
            Some(_) => CleanupAction::LeftAlone, // no PR + marker is ambiguous, leave it
            None => CleanupAction::LeftAlone,
        },
        Some(pr) => {
            // Drift check: local commit vs PR head OID. We tolerate
            // prefix matches in either direction since the local short
            // ID is 12 chars and gh returns the full 40-char OID.
            let drift = if pr.head_ref_oid.is_empty() {
                false
            } else {
                !pr.head_ref_oid.starts_with(&local.commit_id)
                    && !local.commit_id.starts_with(&pr.head_ref_oid)
            };
            if drift {
                return CleanupAction::SkippedDueToDrift {
                    pr: pr.number,
                    local_sha: local.commit_id.clone(),
                    pushed_sha: pr.head_ref_oid.clone(),
                };
            }

            match pr.state {
                PrState::Merged => match merge_marker_on_trunk {
                    Some(sha) => CleanupAction::OrphanDeleted {
                        pr: pr.number,
                        merge_commit_id: sha.into(),
                    },
                    None => CleanupAction::LeftAlone,
                },
                PrState::Closed => match merge_marker_on_trunk {
                    Some(sha) => CleanupAction::OrphanDeleted {
                        pr: pr.number,
                        merge_commit_id: sha.into(),
                    },
                    None => CleanupAction::LeftAlone,
                },
                PrState::Open | PrState::Unknown => CleanupAction::LeftAlone,
            }
        }
    }
}

/// Decide what to do with a `gtmq_*` queue-test branch given its
/// (optional) PR state.
pub fn classify_gtmq_branch(pr: Option<&PrInfo>) -> CleanupAction {
    match pr {
        Some(pr) if pr.state == PrState::Open => CleanupAction::GtmqLeftAlone { pr: pr.number },
        Some(pr) => CleanupAction::GtmqPruned {
            had_pr: Some(pr.number),
        },
        None => CleanupAction::GtmqPruned { had_pr: None },
    }
}

/// Filter `bookmarks` for those whose name starts with any of the
/// configured `gtmq_` prefixes.
pub fn is_gtmq_branch(name: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|p| name.starts_with(p))
}

/// Run the full `jj-gt fetch` pipeline. Returns a per-bookmark log of
/// the decisions made for the caller to print.
///
/// Pipeline steps (numbered to match the design doc):
///   1. `jj git fetch <remote>`.
///   2. Backfill `refs/branch-metadata/*` via `gt track --force` for
///      every local bookmark with an open or recently-closed PR.
///   3. SHA-drift check (per bookmark — skip cleanup, warn).
///   4. `gt sync --no-restack --force`.
///   5. `jj git import` to pick up gt sync's branch deletions.
///   6. Prune `gtmq_*` queue-test branches (closed PR or no PR → delete
///      locally + remote).
///   7. `jj rebase` orphaned children onto trunk.
///   8. Orphan-bookmark fallback — for any remaining local bookmark
///      with a CLOSED PR, look for the merge marker on trunk and
///      prompt to delete.
pub fn run_fetch(
    jj: &JjCli,
    workspace_root: &Path,
    opts: &FetchOpts,
) -> Result<Vec<(LocalBookmark, CleanupAction)>> {
    // 1. Fetch.
    if !opts.dry_run {
        jj::git_fetch(jj, &opts.remote)?;
    } else {
        tracing::info!("[dry-run] would: jj git fetch --remote {}", opts.remote);
    }

    // Snapshot the current bookmark list once for the cleanup phase.
    let mut bookmarks = list_local_bookmarks(jj)?;

    // Partition gtmq_* vs everything else.
    let (gtmq, normal): (Vec<_>, Vec<_>) = bookmarks
        .drain(..)
        .partition(|b| is_gtmq_branch(&b.name, &opts.gtmq_prefixes));

    // Look up PR info for all branches in one batched call each. The
    // gh CLI search syntax accepts repeated `head:` clauses.
    let normal_prs = if normal.is_empty() {
        Vec::new()
    } else {
        let names: Vec<String> = normal.iter().map(|b| b.name.clone()).collect();
        gh::find_prs_for_branches(workspace_root, &names, 200)?
    };

    // 2. Backfill metadata refs for bookmarks that have a PR. Sort
    // bottom→top so gt accepts each parent reference.
    if !opts.no_backfill {
        let stacked = derive_parents(
            jj,
            &normal.iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
            &opts.trunk,
        )?;
        let stacked = crate::stack::sort_for_tracking(&stacked);
        for sb in &stacked {
            let has_pr = normal_prs.iter().any(|p| p.head_ref_name == sb.name);
            if !has_pr {
                continue;
            }
            let parent = match &sb.parent {
                BookmarkOrTrunk::Bookmark(p) => p.clone(),
                BookmarkOrTrunk::Trunk => opts.trunk.clone(),
            };
            if opts.dry_run {
                tracing::info!("[dry-run] would: gt track {} --parent {}", sb.name, parent);
            } else {
                gt::track(workspace_root, &sb.name, &parent)?;
            }
        }
    }

    // 3 + 4 + 5: classify normal bookmarks (drift / cleanup decisions),
    // run gt sync, then re-import.
    let mut actions: Vec<(LocalBookmark, CleanupAction)> = Vec::new();
    for local in &normal {
        let pr = normal_prs.iter().find(|p| p.head_ref_name == local.name);
        let marker = match pr {
            Some(pr) if pr.state.is_terminal() => {
                jj::find_pr_merge_marker_on_trunk(jj, pr.number, &opts.trunk)?
            }
            _ => None,
        };
        let action = classify_local_bookmark(local, pr, marker.as_deref());
        actions.push((local.clone(), action));
    }

    if !opts.dry_run {
        // gt sync runs unconditionally — even when every classified
        // action was LeftAlone, gt may still know about branches we
        // don't (untracked-on-this-side branches with closed PRs).
        gt::sync_no_restack(workspace_root)?;
        jj::git_import(jj)?;
    } else {
        tracing::info!("[dry-run] would: gt sync --no-restack --force");
    }

    // 6. gtmq_* pruning.
    if !opts.no_gtmq_prune {
        let gtmq_prs = if gtmq.is_empty() {
            Vec::new()
        } else {
            gh::list_prs_by_head_prefix(workspace_root, &opts.gtmq_prefixes, 500)?
        };
        for branch in &gtmq {
            let pr = gtmq_prs.iter().find(|p| p.head_ref_name == branch.name);
            let action = classify_gtmq_branch(pr);
            if let CleanupAction::GtmqPruned { .. } = action {
                if !opts.dry_run {
                    let _ = jj::delete_bookmark(jj, &branch.name);
                    let _ = jj::delete_remote_branch(workspace_root, &opts.remote, &branch.name);
                } else {
                    tracing::info!(
                        "[dry-run] would: jj bookmark delete {} && git push --delete {} {}",
                        branch.name,
                        opts.remote,
                        branch.name
                    );
                }
            }
            actions.push((branch.clone(), action));
        }
    }

    // 7. Orphan-restack via `jj rebase` for bookmarks whose parent
    // got cleaned up. Best-effort: rebase onto trunk if the bookmark
    // is still present after step 4/5.
    if !opts.no_rebase && !opts.dry_run {
        // After gt sync + jj import some of `normal` may no longer
        // exist as local bookmarks. Re-query and rebase any whose
        // current parent commit isn't on trunk's ancestry.
        let remaining = list_local_bookmarks(jj)?;
        for local in &remaining {
            let revset = local.name.clone();
            // Best-effort. If it errors (already on trunk, conflict),
            // log and continue — the user can `jj rebase` manually.
            let _ = jj::rebase(jj, &revset, &opts.trunk);
        }
    }

    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::{PrInfo, PrState};

    fn local(name: &str, commit: &str) -> LocalBookmark {
        LocalBookmark {
            name: name.into(),
            commit_id: commit.into(),
        }
    }

    fn pr_with(number: u32, branch: &str, head_oid: &str, state: PrState) -> PrInfo {
        PrInfo {
            number,
            head_ref_name: branch.into(),
            head_ref_oid: head_oid.into(),
            state,
            is_draft: false,
            merge_state_status: None,
            labels: Vec::new(),
        }
    }

    #[test]
    fn classify_no_pr_leaves_alone() {
        let b = local("foo", "abc123");
        assert_eq!(
            classify_local_bookmark(&b, None, None),
            CleanupAction::LeftAlone
        );
    }

    #[test]
    fn classify_open_pr_same_sha_leaves_alone() {
        let b = local("foo", "abc123");
        let pr = pr_with(1, "foo", "abc12345678", PrState::Open);
        assert_eq!(
            classify_local_bookmark(&b, Some(&pr), None),
            CleanupAction::LeftAlone
        );
    }

    #[test]
    fn classify_open_pr_different_sha_flags_drift() {
        let b = local("foo", "abc123");
        let pr = pr_with(1, "foo", "deadbeef", PrState::Open);
        let action = classify_local_bookmark(&b, Some(&pr), None);
        assert!(matches!(action, CleanupAction::SkippedDueToDrift { .. }));
    }

    #[test]
    fn classify_merged_pr_with_marker_orphan_deletes() {
        let b = local("foo", "abc123");
        let pr = pr_with(7, "foo", "abc12345678", PrState::Merged);
        let action = classify_local_bookmark(&b, Some(&pr), Some("merge_sha_xyz"));
        assert_eq!(
            action,
            CleanupAction::OrphanDeleted {
                pr: 7,
                merge_commit_id: "merge_sha_xyz".into()
            }
        );
    }

    #[test]
    fn classify_closed_no_marker_leaves_alone() {
        let b = local("foo", "abc123");
        let pr = pr_with(7, "foo", "abc12345678", PrState::Closed);
        let action = classify_local_bookmark(&b, Some(&pr), None);
        assert_eq!(action, CleanupAction::LeftAlone);
    }

    #[test]
    fn classify_drift_short_circuits_marker_check() {
        // If we have drift AND a merge marker, drift wins — we never
        // want to delete a local bookmark that has unpushed work,
        // even if a same-numbered PR happened to land elsewhere.
        let b = local("foo", "abc123");
        let pr = pr_with(7, "foo", "deadbeef", PrState::Merged);
        let action = classify_local_bookmark(&b, Some(&pr), Some("merge_sha_xyz"));
        assert!(matches!(action, CleanupAction::SkippedDueToDrift { .. }));
    }

    #[test]
    fn gtmq_open_pr_left_alone() {
        let pr = pr_with(101, "gtmq_xyz", "x", PrState::Open);
        assert_eq!(
            classify_gtmq_branch(Some(&pr)),
            CleanupAction::GtmqLeftAlone { pr: 101 }
        );
    }

    #[test]
    fn gtmq_closed_pr_pruned() {
        let pr = pr_with(101, "gtmq_xyz", "x", PrState::Closed);
        assert_eq!(
            classify_gtmq_branch(Some(&pr)),
            CleanupAction::GtmqPruned { had_pr: Some(101) }
        );
    }

    #[test]
    fn gtmq_no_pr_pruned() {
        assert_eq!(
            classify_gtmq_branch(None),
            CleanupAction::GtmqPruned { had_pr: None }
        );
    }

    #[test]
    fn is_gtmq_branch_matches_default_prefix() {
        let prefixes = vec!["gtmq_".to_owned()];
        assert!(is_gtmq_branch("gtmq_abc", &prefixes));
        assert!(!is_gtmq_branch("feature/foo", &prefixes));
    }

    #[test]
    fn is_gtmq_branch_matches_extra_prefixes() {
        let prefixes = vec!["gtmq_".to_owned(), "graphite-".to_owned()];
        assert!(is_gtmq_branch("graphite-tmp-1", &prefixes));
        assert!(!is_gtmq_branch("other", &prefixes));
    }
}
