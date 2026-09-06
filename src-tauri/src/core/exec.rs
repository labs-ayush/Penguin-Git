use std::path::Path;
use std::process::Command;

/// The single place in the whole codebase allowed to spawn a `git` subprocess.
/// Every git operation added in later phases must go through this function —
/// never call `std::process::Command::new("git")` anywhere else.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to execute git: {0}")]
    Spawn(#[from] std::io::Error),

    #[error("git command failed (exit code {exit_code:?}): {stderr}")]
    CommandFailed {
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("git produced invalid UTF-8 output: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    #[error("validation error: {0}")]
    ValidationError(String),
}

/// A completed git invocation, including runs that exited non-zero.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Runs `git <args>` in `cwd` and returns stdout on success.
///
/// On a non-zero exit code, returns `GitError::CommandFailed` carrying the
/// process's exit code and trimmed stderr — callers should not need to
/// re-parse stderr text to distinguish failure modes; add a more specific
/// variant here if a later phase needs to branch on a particular git error.
pub fn run_git(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    let output = run_git_raw(cwd, args)?;

    if output.success() {
        Ok(output.stdout)
    } else {
        Err(GitError::CommandFailed {
            exit_code: output.exit_code,
            stderr: output.stderr,
        })
    }
}

/// Runs `git <args>` and hands back the result whatever the exit code.
///
/// For the subcommands where a non-zero exit is a *result* rather than a
/// failure — `diff` with `--exit-code`/`--no-index` exits 1 to mean "there are
/// differences", `merge` exits 1 on conflicts — and stdout still matters. Both
/// this and [`run_git`] funnel through the one `Command::new("git")` below, so
/// git invocation stays auditable in a single place.
pub fn run_git_raw(cwd: &Path, args: &[&str]) -> Result<GitOutput, GitError> {
    run_git_raw_with_env(cwd, args, &[])
}

/// Runs `git <args>` and hands back the result whatever the exit code, passing custom environment variables.
pub fn run_git_raw_with_env(
    cwd: &Path,
    args: &[&str],
    envs: &[(&str, &std::ffi::OsStr)],
) -> Result<GitOutput, GitError> {
    let mut cmd = create_git_command(cwd, args);

    for (k, v) in envs {
        cmd.env(k, v);
    }

    let output = cmd.output()?;

    Ok(GitOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        exit_code: output.status.code(),
    })
}

/// Helper function to create a git Command with filtered/whitelisted environment variables.
fn create_git_command(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd).args(args);
    cmd.env_clear();

    const WHITELIST: &[&str] = &[
        "path",
        "home",
        "userprofile",
        "homedrive",
        "homepath",
        "user",
        "username",
        "logname",
        "ssh_auth_sock",
        "ssh_askpass",
        "git_askpass",
        "git_terminal_prompt",
        "ssh_askpass_require",
        "git_author_name",
        "git_author_email",
        "git_author_date",
        "git_committer_name",
        "git_committer_email",
        "git_committer_date",
        "git_config_nosystem",
        "git_config_parameters",
        "git_dir",
        "git_work_tree",
        "git_index_file",
        "git_object_directory",
        "git_alternate_object_directories",
        "git_common_dir",
        "git_exec_path",
        "git_template_dir",
        "git_namespace",
        "lang",
        "lc_all",
        "lc_ctype",
        "lc_messages",
        "lc_collate",
        "lc_numeric",
        "lc_time",
        "tz",
        "term",
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "all_proxy",
        "git_http_user_agent",
        "git_trace",
        "git_trace_pack_access",
        "git_trace_packet",
        "git_trace_performance",
        "git_trace_setup",
        "git_trace_shallow",
        "git_ssh",
        "git_ssh_command",
        "git_ssh_variant",
        "git_ssl_cainfo",
        "git_ssl_capath",
        "git_ssl_no_verify",
        "git_ssl_cert",
        "git_ssl_key",
        "git_flush",
        "git_curl_verbose",
        "systemroot",
        "systemdrive",
        "windir",
        "comspec",
        "pathext",
        "temp",
        "tmp",
        "appdata",
        "localappdata",
    ];

    for (key, val) in std::env::vars_os() {
        if let Some(key_str) = key.to_str() {
            let key_lower = key_str.to_lowercase();
            if WHITELIST.contains(&key_lower.as_str()) {
                cmd.env(key, val);
            }
        }
    }

    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never");

    cmd
}

/// Runs `git <args>` piping `stdin_data` to the child's stdin, returns stdout
/// on success.
///
/// Needed for commands like `git apply -` and `git am` that read patch data
/// from stdin rather than a file path argument.
pub fn run_git_with_stdin(
    cwd: &Path,
    args: &[&str],
    stdin_data: &[u8],
) -> Result<String, GitError> {
    let output = run_git_raw_with_stdin(cwd, args, stdin_data)?;

    if output.success() {
        Ok(output.stdout)
    } else {
        Err(GitError::CommandFailed {
            exit_code: output.exit_code,
            stderr: output.stderr,
        })
    }
}

/// Like [`run_git_raw`] but pipes `stdin_data` to the child process.
pub fn run_git_raw_with_stdin(
    cwd: &Path,
    args: &[&str],
    stdin_data: &[u8],
) -> Result<GitOutput, GitError> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = create_git_command(cwd, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        // Write all data then drop to close the pipe so git sees EOF.
        stdin.write_all(stdin_data)?;
    }

    let output = child.wait_with_output()?;

    Ok(GitOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        exit_code: output.status.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_support::FixtureRepo;

    #[test]
    fn run_git_version_succeeds() {
        let repo = FixtureRepo::new();
        let out = run_git(repo.path(), &["--version"]).expect("git --version should succeed");
        assert!(out.starts_with("git version"));
    }

    #[test]
    fn run_git_reports_failure_with_stderr() {
        let repo = FixtureRepo::new();
        let err = run_git(repo.path(), &["not-a-real-git-command"])
            .expect_err("an invalid git subcommand should fail");
        match err {
            GitError::CommandFailed { stderr, .. } => {
                assert!(!stderr.is_empty(), "stderr should be captured on failure");
            }
            other => panic!("expected CommandFailed, got {other:?}"),
        }
    }

    #[test]
    fn a_configured_ssh_command_is_left_intact() {
        // The whole point of shelling out to system git is that the user's own
        // configuration keeps working. Overriding `core.sshCommand` — which is
        // what setting `GIT_SSH_COMMAND` would do — silently breaks custom keys,
        // proxy commands, and non-OpenSSH clients.
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        repo.git(&["config", "core.sshCommand", "/usr/bin/my-custom-ssh -v"]);

        let configured = run_git(repo.path(), &["config", "--get", "core.sshCommand"])
            .expect("reading the config back should succeed");

        assert_eq!(configured.trim(), "/usr/bin/my-custom-ssh -v");
    }

    #[test]
    fn run_git_log_on_fixture_repo() {
        let repo = FixtureRepo::new();
        repo.commit("first.txt", "hello", "Initial commit");
        let out = run_git(repo.path(), &["log", "--oneline"]).expect("log should succeed");
        assert!(out.contains("Initial commit"));
    }

    #[test]
    fn run_git_failure_carries_the_exit_code() {
        let repo = FixtureRepo::new();
        // `rev-parse` on a ref that doesn't exist exits 128, not 1 — callers that
        // branch on failure mode need the number, not just "it failed".
        let err = run_git(repo.path(), &["rev-parse", "--verify", "no-such-ref"])
            .expect_err("an unknown ref should fail");
        match err {
            GitError::CommandFailed { exit_code, .. } => {
                assert_eq!(exit_code, Some(128));
            }
            other => panic!("expected CommandFailed, got {other:?}"),
        }
    }

    #[test]
    fn run_git_raw_hands_back_a_non_zero_exit_instead_of_erroring() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "one\n", "Initial commit");
        repo.write("a.txt", "two\n");

        // `diff --exit-code` exits 1 to mean "there are differences" — a result,
        // not a failure. `run_git` would turn that into an error and throw the
        // diff away; `run_git_raw` is what keeps both.
        let output = run_git_raw(repo.path(), &["diff", "--exit-code", "--no-color"])
            .expect("spawning git should succeed even when git exits non-zero");

        assert_eq!(output.exit_code, Some(1));
        assert!(!output.success());
        assert!(
            output.stdout.contains("+two"),
            "stdout must survive a non-zero exit, got: {}",
            output.stdout
        );
    }

    #[test]
    fn git_output_reports_success_only_for_exit_zero() {
        let base = GitOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
        };
        assert!(base.success());
        assert!(!GitOutput {
            exit_code: Some(1),
            ..base.clone()
        }
        .success());
        assert!(
            !GitOutput {
                exit_code: None,
                ..base
            }
            .success(),
            "a process killed by a signal has no exit code and is not a success"
        );
    }

    #[test]
    fn non_utf8_output_is_read_lossily_rather_than_failing() {
        let repo = FixtureRepo::new();
        repo.commit("seed.txt", "x", "Initial commit");
        // Latin-1 bytes: valid in a real file, invalid as UTF-8. A diff viewer
        // that errors out on legacy-encoded files is useless.
        std::fs::write(repo.file_path("legacy.txt"), [0xC0, 0xC1, 0xFF, b'\n'])
            .expect("write raw bytes");
        repo.git(&["add", "legacy.txt"]);

        let output = run_git_raw(repo.path(), &["diff", "--cached", "--no-color"])
            .expect("a diff containing invalid UTF-8 must not fail the whole operation");

        assert!(output.success());
        assert!(output.stdout.contains("legacy.txt"));
    }

    #[test]
    fn interactive_prompting_is_disabled_in_the_child_environment() {
        // A `!` alias runs through the shell, which is the only way to observe
        // the environment git actually hands its children. Without this, a
        // credential prompt would block the GUI forever with no terminal to
        // answer on — the hang has no recovery path from the UI.
        let repo = FixtureRepo::new();
        let probe = run_git(
            repo.path(),
            &[
                "-c",
                "alias.probe=!echo prompt=$GIT_TERMINAL_PROMPT askpass=[$GIT_ASKPASS] ssh=[$SSH_ASKPASS_REQUIRE]",
                "probe",
            ],
        )
        .expect("alias probe should run");

        assert!(
            probe.contains("prompt=0"),
            "GIT_TERMINAL_PROMPT must be 0, got: {probe}"
        );
        assert!(
            probe.contains("askpass=[]"),
            "GIT_ASKPASS must be blanked, got: {probe}"
        );
        assert!(
            probe.contains("ssh=[never]"),
            "SSH_ASKPASS_REQUIRE must be never, got: {probe}"
        );
        assert!(
            !probe.contains("GIT_SSH_COMMAND"),
            "GIT_SSH_COMMAND must never be set — it overrides core.sshCommand"
        );
    }

    #[test]
    fn run_git_with_stdin_pipes_data_to_git() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "line1\n", "Initial commit");
        repo.write("a.txt", "line1\nline2\n");
        repo.git(&["add", "a.txt"]);

        // Generate a patch from the staged change.
        let patch =
            run_git(repo.path(), &["diff", "--cached", "--no-color"]).expect("diff should succeed");

        // Reset the index and working tree so the patch can be re-applied.
        repo.git(&["reset", "--hard", "HEAD"]);

        // Apply the patch via stdin — this is the path used by Import Patch.
        let result = run_git_with_stdin(repo.path(), &["apply", "--check"], patch.as_bytes());
        assert!(result.is_ok(), "git apply --check via stdin should succeed");
    }

    #[test]
    fn run_git_raw_with_stdin_returns_output_on_failure() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "hello\n", "Initial commit");

        // Feed garbage to git apply — it should fail but not panic.
        let output =
            run_git_raw_with_stdin(repo.path(), &["apply", "--check"], b"not a valid patch\n")
                .expect("spawning should succeed even when git rejects the input");

        assert!(!output.success());
        assert!(!output.stderr.is_empty());
    }

    #[test]
    fn run_git_does_not_leak_unwhitelisted_env_vars() {
        let repo = FixtureRepo::new();
        std::env::set_var("PENGUIN_GIT_SECRET_TOKEN", "super-secret-value");

        let probe = run_git(
            repo.path(),
            &[
                "-c",
                "alias.probe=!echo secret=[$PENGUIN_GIT_SECRET_TOKEN] path=[$PATH]",
                "probe",
            ],
        )
        .expect("alias probe should run");

        assert!(
            probe.contains("secret=[]"),
            "un-whitelisted secret was leaked! got: {probe}"
        );
        assert!(
            !probe.contains("path=[]"),
            "PATH was not passed through! got: {probe}"
        );

        std::env::remove_var("PENGUIN_GIT_SECRET_TOKEN");
    }
}
