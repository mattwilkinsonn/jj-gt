use std::path::Path;
use std::process::Command;

// Remote bookmarks must stay mutable in fixtures even with hostile user config.
// Keep the override repo-local so JjCli retains the inherited environment.
pub fn init_jj_repo(cwd: &Path) {
    for args in [
        &["git", "init", "--colocate"][..],
        &[
            "config",
            "set",
            "--repo",
            "revset-aliases.\"immutable_heads()\"",
            "none()",
        ][..],
    ] {
        let out = Command::new("jj")
            .env("JJ_CONFIG", "/dev/null")
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
}
