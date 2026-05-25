//! Library entrypoint for the `jj-gt` binary.
//!
//! Exposes the same surface as a library so downstream tools could
//! depend on jj-gt's stack derivation + cleanup classifier without
//! shelling out (mirror of how jj-gt itself depends on `jj_hooks`).

pub mod cleanup;
pub mod cli;
pub mod completions;
pub mod error;
pub mod gh;
pub mod gt;
pub mod hooks;
pub mod init;
pub mod jj;
pub mod select;
pub mod stack;
pub mod status;

// Re-export the runner enum so downstream consumers can construct a
// HookOpts without taking a transitive `jj_hooks` dep.
pub use jj_hooks::runner::Runner;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::error::JjGtError;
use crate::jj::JjCli;

/// Parse CLI args, dispatch to a subcommand, and return the process
/// exit code. `bin/jj-gt` is a trivial wrapper around this function.
pub fn run() -> ExitCode {
    // Handle dynamic completion requests *before* anything else.
    use clap::CommandFactory;
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log_level)),
        )
        .with_target(false)
        .without_time()
        .try_init();

    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("jj-gt: {e}");
            ExitCode::from(1)
        }
    }
}

fn dispatch(cli: Cli) -> Result<ExitCode, JjGtError> {
    let jj = JjCli::new(std::env::current_dir()?);

    match cli.command {
        Command::Submit {
            bookmarks,
            submit,
            trunk,
            no_export,
            no_restore_cwc,
            no_hooks,
            hk_runner,
        } => submit_cmd(
            &jj,
            bookmarks,
            submit,
            trunk,
            no_export,
            no_restore_cwc,
            no_hooks,
            hk_runner,
        ),

        Command::Track {
            bookmarks,
            trunk,
            no_export,
            parent,
            dry_run,
        } => track_cmd(&jj, bookmarks, trunk, no_export, parent, dry_run),

        Command::Fetch {
            remote,
            trunk,
            no_backfill,
            no_rebase,
            no_gtmq_prune,
            gtmq_prefix,
            auto,
            dry_run,
        } => fetch_cmd(
            &jj,
            remote,
            trunk,
            no_backfill,
            no_rebase,
            no_gtmq_prune,
            gtmq_prefix,
            auto,
            dry_run,
        ),

        Command::Status {
            bookmarks,
            trunk,
            json,
        } => status_cmd(&jj, bookmarks, trunk, json),

        Command::Log { trunk } => log_cmd(&jj, trunk),

        Command::Init => {
            init::print_init();
            Ok(ExitCode::SUCCESS)
        }

        Command::Completions { shell } => completions_cmd(shell),
    }
}

#[allow(clippy::too_many_arguments)]
fn submit_cmd(
    jj: &JjCli,
    bookmarks: cli::BookmarkArgs,
    submit: cli::SubmitArgs,
    trunk: Option<String>,
    no_export: bool,
    no_restore_cwc: bool,
    no_hooks: bool,
    hk_runner: Option<cli::RunnerArg>,
) -> Result<ExitCode, JjGtError> {
    let workspace_root = jj.workspace_root().map_err(JjGtError::Hooks)?;
    let trunk = status::resolve_trunk(&workspace_root, trunk.as_deref())?;

    // 3 + 4. Resolve bookmark selection, then derive parents.
    let selected = select::resolve_bookmarks(jj, &bookmarks, &trunk)?;
    if selected.is_empty() {
        return Err(JjGtError::NoBookmarksSelected);
    }
    let stacked = stack::derive_parents(jj, &selected, &trunk)?;
    let tip = stack::find_tip(&stacked)?;

    // 1. Export jj bookmarks → git refs (idempotent).
    if !no_export {
        jj::git_export(jj)?;
    }

    // 5. Hook gate against trunk..tip.
    if !no_hooks {
        let opts = hooks::HookOpts {
            runner_override: hk_runner.map(Into::into),
        };
        hooks::run_pre_push(jj, &workspace_root, &trunk, &tip, &opts)?;
    }

    // 6. gt track per (bookmark, parent). Must be bottom→top because
    // `gt track <child> --parent <parent>` errors if `<parent>` isn't
    // already tracked.
    let stacked_sorted = stack::sort_for_tracking(&stacked);
    for sb in &stacked_sorted {
        let parent = sb.parent.as_branch_name(&trunk);
        if submit.dry_run {
            println!("would: gt track {} --parent {}", sb.name, parent);
        } else {
            gt::track(&workspace_root, &sb.name, parent)?;
        }
    }

    // 7. Record @ for restoration.
    let saved_change_id = if no_restore_cwc {
        None
    } else {
        Some(jj::current_change_id(jj)?)
    };

    // 8. gt submit --stack --branch <tip>.
    let argv = gt::build_submit_argv(&tip, &submit);
    if submit.dry_run {
        println!("would: gt {}", argv.join(" "));
    } else {
        gt::submit(&workspace_root, &argv)?;
    }

    // 9. Track each pushed bookmark so jj's `untracked_remote_bookmarks()`
    // (part of the default `immutable_heads()` revset) doesn't freeze
    // the commits we just submitted. See jj::track_bookmark_on_remote
    // for the full rationale; the short version is: `gt submit`
    // shells out to `git push`, jj never sees that push, and the
    // next jj operation imports the new `refs/remotes/<remote>/*`
    // refs as untracked → ancestors flip immutable → users can't
    // amend their just-pushed commits.
    if !submit.dry_run {
        for sb in &stacked_sorted {
            if let Err(e) = jj::track_bookmark_on_remote(jj, &sb.name, &bookmarks.remote) {
                // Tracking is best-effort: even if a single bookmark
                // fails (e.g. some race with a concurrent fetch), the
                // submit itself already succeeded. Log and continue
                // rather than 1-exit a finished submit.
                eprintln!(
                    "jj-gt: warning: could not track `{}@{}`: {e}",
                    sb.name, bookmarks.remote
                );
            }
        }
    }

    // 10. Restore @.
    if let Some(change) = saved_change_id
        && !submit.dry_run
    {
        // Best-effort: if @ no longer maps to something checkout-
        // able (rare — gt's import shouldn't abandon the change),
        // log and continue.
        if let Err(e) = jj::edit_change(jj, &change) {
            eprintln!("jj-gt: could not restore @ to {change}: {e}");
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn track_cmd(
    jj: &JjCli,
    bookmarks: cli::BookmarkArgs,
    trunk: Option<String>,
    no_export: bool,
    parent_override: Option<String>,
    dry_run: bool,
) -> Result<ExitCode, JjGtError> {
    let workspace_root = jj.workspace_root().map_err(JjGtError::Hooks)?;
    let trunk = status::resolve_trunk(&workspace_root, trunk.as_deref())?;

    let selected = select::resolve_bookmarks(jj, &bookmarks, &trunk)?;
    if selected.is_empty() {
        return Err(JjGtError::NoBookmarksSelected);
    }

    if !no_export {
        jj::git_export(jj)?;
    }

    let pairs: Vec<(String, String)> = if let Some(p) = parent_override {
        selected.iter().map(|b| (b.clone(), p.clone())).collect()
    } else {
        let stacked = stack::derive_parents(jj, &selected, &trunk)?;
        // Same ordering requirement as the submit path — gt rejects
        // tracking a child whose parent isn't tracked yet.
        let sorted = stack::sort_for_tracking(&stacked);
        sorted
            .into_iter()
            .map(|sb| {
                let parent = sb.parent.as_branch_name(&trunk).to_owned();
                (sb.name, parent)
            })
            .collect()
    };

    for (bookmark, parent) in pairs {
        if dry_run {
            println!("would: gt track {bookmark} --parent {parent}");
        } else {
            gt::track(&workspace_root, &bookmark, &parent)?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::too_many_arguments)]
fn fetch_cmd(
    jj: &JjCli,
    remote: String,
    trunk: Option<String>,
    no_backfill: bool,
    no_rebase: bool,
    no_gtmq_prune: bool,
    gtmq_prefix: Vec<String>,
    auto: bool,
    dry_run: bool,
) -> Result<ExitCode, JjGtError> {
    let workspace_root = jj.workspace_root().map_err(JjGtError::Hooks)?;
    let trunk = status::resolve_trunk(&workspace_root, trunk.as_deref())?;

    let prefixes = if gtmq_prefix.is_empty() {
        vec!["gtmq_".into()]
    } else {
        gtmq_prefix
    };

    let opts = cleanup::FetchOpts {
        remote,
        trunk,
        no_backfill,
        no_rebase,
        no_gtmq_prune,
        gtmq_prefixes: prefixes,
        auto,
        dry_run,
    };

    let actions = cleanup::run_fetch(jj, &workspace_root, &opts)?;
    for (bookmark, action) in &actions {
        println!("  {} → {:?}", bookmark.name, action);
    }

    Ok(ExitCode::SUCCESS)
}

fn status_cmd(
    jj: &JjCli,
    bookmarks: cli::BookmarkArgs,
    trunk: Option<String>,
    json: bool,
) -> Result<ExitCode, JjGtError> {
    let workspace_root = jj.workspace_root().map_err(JjGtError::Hooks)?;
    let trunk = status::resolve_trunk(&workspace_root, trunk.as_deref())?;

    let selected = select::resolve_bookmarks(jj, &bookmarks, &trunk)?;
    if selected.is_empty() {
        return Err(JjGtError::NoBookmarksSelected);
    }
    let stacked = stack::derive_parents(jj, &selected, &trunk)?;

    let locals = status::collect_local_commits(jj, &selected)?;
    let prs = status::fetch_pr_info(&workspace_root, &selected)?;

    let out = status::build(&trunk, &stacked, &locals, &prs);
    if json {
        println!("{}", status::render_json(&out)?);
    } else {
        print!("{}", status::render_table(&out));
    }
    Ok(ExitCode::SUCCESS)
}

fn log_cmd(jj: &JjCli, trunk: Option<String>) -> Result<ExitCode, JjGtError> {
    let workspace_root = jj.workspace_root().map_err(JjGtError::Hooks)?;
    let trunk = status::resolve_trunk(&workspace_root, trunk.as_deref())?;
    let args = cli::BookmarkArgs {
        all: true,
        remote: "origin".into(),
        ..cli::BookmarkArgs::default()
    };
    let selected = select::resolve_bookmarks(jj, &args, &trunk)?;
    let stacked = stack::derive_parents(jj, &selected, &trunk)?;

    println!("trunk: {trunk}");
    println!("stack (top → bottom):");
    for sb in stacked.iter().rev() {
        let parent = sb.parent.as_branch_name(&trunk);
        println!("  ● {}\n    └─ parent: {}", sb.name, parent);
    }
    Ok(ExitCode::SUCCESS)
}

fn completions_cmd(shell: clap_complete::Shell) -> Result<ExitCode, JjGtError> {
    use clap::CommandFactory;
    use clap_complete::env::EnvCompleter;
    use clap_complete::env::{Bash, Elvish, Fish, Powershell, Zsh};

    let cmd = Cli::command();
    let bin_name = std::env::args()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "jj-gt".into());

    let mut out = std::io::stdout();
    let result = match shell {
        clap_complete::Shell::Bash => {
            Bash.write_registration("COMPLETE", &bin_name, &bin_name, &bin_name, &mut out)
        }
        clap_complete::Shell::Zsh => {
            Zsh.write_registration("COMPLETE", &bin_name, &bin_name, &bin_name, &mut out)
        }
        clap_complete::Shell::Fish => {
            Fish.write_registration("COMPLETE", &bin_name, &bin_name, &bin_name, &mut out)
        }
        clap_complete::Shell::PowerShell => {
            Powershell.write_registration("COMPLETE", &bin_name, &bin_name, &bin_name, &mut out)
        }
        clap_complete::Shell::Elvish => {
            Elvish.write_registration("COMPLETE", &bin_name, &bin_name, &bin_name, &mut out)
        }
        _ => {
            eprintln!("jj-gt: unsupported shell for dynamic completion");
            return Ok(ExitCode::from(2));
        }
    };
    let _ = cmd;
    result.map_err(JjGtError::Io)?;
    Ok(ExitCode::SUCCESS)
}
