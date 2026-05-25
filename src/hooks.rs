//! Thin wrapper around [`jj_hooks::run_for_revset_outcome`] that
//! translates jj-gt's CLI inputs (`--hk-runner`, `--no-hooks`) into the
//! library call and surfaces failures with a jj-gt-flavoured error
//! message.

use std::path::Path;

use jj_hooks::jj::JjCli;
use jj_hooks::runner::{Runner, Stage};

use crate::error::{JjGtError, Result};

#[derive(Debug, Clone, Default)]
pub struct HookOpts {
    /// Override the autodetected hook runner. `None` means "let
    /// jj_hooks autodetect from the target commit's tree".
    pub runner_override: Option<Runner>,
}

/// Run pre-push hooks against the `<trunk>..<tip>` revset.
///
/// `<tip>` is the bookmark at the head of the selected stack;
/// `<trunk>` is the resolved trunk name (typically `main`). The
/// resulting diff range matches what `git push` *should* be running
/// hooks against — but doesn't, in a colocated jj workspace, because
/// HEAD points at the parent branch and the `origin/main..HEAD` diff
/// is empty.
///
/// Returns Ok(()) on success; Err with a descriptive message on
/// failure so the caller can surface it and abort before invoking gt.
pub fn run_pre_push(
    jj: &JjCli,
    workspace_root: &Path,
    trunk: &str,
    tip: &str,
    opts: &HookOpts,
) -> Result<()> {
    let revset = pre_push_revset(trunk, tip);
    let outcome = jj_hooks::run_for_revset_outcome(
        jj,
        workspace_root,
        opts.runner_override,
        Stage::PrePush,
        &revset,
    )?;

    match outcome {
        None => {
            // Empty revset — nothing new to push, nothing to gate.
            // Don't fail the pipeline; gt will figure out there's
            // nothing to submit too.
            tracing::info!("pre-push: revset `{revset}` is empty; skipping hooks");
            Ok(())
        }
        Some(o) if o.success && o.fixup_commit.is_none() => Ok(()),
        Some(o) if !o.success => Err(JjGtError::Invalid(format!(
            "pre-push hooks failed for revset `{revset}`"
        ))),
        Some(o) => {
            // success && fixup_commit.is_some() — hooks auto-fixed
            // something. Make the user re-run after squashing the fixup
            // into the bookmark so we push the fixed code, not the
            // pre-fix code.
            let commit = o.fixup_commit.unwrap_or_else(|| "<unknown>".into());
            Err(JjGtError::Invalid(format!(
                "pre-push hooks modified files (fixup commit {commit}); \
                 squash it into the relevant bookmark and re-run `jj-gt submit`"
            )))
        }
    }
}

pub fn pre_push_revset(trunk: &str, tip: &str) -> String {
    format!("{trunk}..{tip}")
}

#[cfg(test)]
mod tests {
    use super::pre_push_revset;

    #[test]
    fn revset_is_trunk_dotdot_tip() {
        assert_eq!(pre_push_revset("main", "top--athena"), "main..top--athena");
    }
}
