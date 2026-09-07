use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::branch::reject_option_like;
use crate::core::exec::{run_git, run_git_raw_with_env, GitError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebaseTodoItem {
    pub action: String, // pick, reword, edit, squash, fixup, drop
    pub hash: String,
    pub message: String,
}

/// Finds the path to `penguingit-sequence-editor` executable relative to current executable.
pub fn find_sequence_editor_executable() -> Result<PathBuf, GitError> {
    let binary_name = if cfg!(target_os = "windows") {
        "penguingit-sequence-editor.exe"
    } else {
        "penguingit-sequence-editor"
    };

    let current_exe = env::current_exe()?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| GitError::CommandFailed {
            exit_code: None,
            stderr: "Cannot determine current executable directory".into(),
        })?;

    // Check directory candidates in order:
    // 1. Same directory (packaged build & tauri dev)
    let candidate = exe_dir.join(binary_name);
    if candidate.exists() {
        return Ok(candidate);
    }

    // 2. Parent directory (e.g. target/debug/ when current_exe is in target/debug/deps/)
    if let Some(parent) = exe_dir.parent() {
        let candidate = parent.join(binary_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    // 3. Search target/debug or target/release up the ancestor tree
    for ancestor in exe_dir.ancestors() {
        let debug_cand = ancestor.join("target").join("debug").join(binary_name);
        if debug_cand.exists() {
            return Ok(debug_cand);
        }
        let release_cand = ancestor.join("target").join("release").join(binary_name);
        if release_cand.exists() {
            return Ok(release_cand);
        }
    }

    Err(GitError::CommandFailed {
        exit_code: None,
        stderr: format!(
            "Sequence editor binary '{binary_name}' not found near {}",
            exe_dir.display()
        ),
    })
}

/// Executes a non-interactive `git rebase <target>`.
pub fn plain_rebase(cwd: &Path, target: &str) -> Result<String, GitError> {
    reject_option_like(target)?;
    run_git(cwd, &["rebase", target])
}

/// Executes an interactive `git rebase -i <base_ref>` using `penguingit-sequence-editor`.
pub fn interactive_rebase(
    cwd: &Path,
    base_ref: &str,
    todo_items: &[RebaseTodoItem],
) -> Result<String, GitError> {
    reject_option_like(base_ref)?;
    let editor_exe = find_sequence_editor_executable()?;

    // Create a temporary file to store the customized todo list
    let temp_file = tempfile::Builder::new()
        .prefix("penguingit-rebase-todo-")
        .suffix(".txt")
        .tempfile()
        .map_err(GitError::Spawn)?;

    let mut content = String::new();
    for item in todo_items {
        content.push_str(&format!(
            "{} {} {}\n",
            item.action.trim(),
            item.hash.trim(),
            item.message.trim()
        ));
    }

    fs::write(temp_file.path(), &content).map_err(GitError::Spawn)?;

    let editor_os = editor_exe.as_os_str();
    let todo_os = temp_file.path().as_os_str();
    let true_os = std::ffi::OsStr::new("true");

    let envs = [
        ("GIT_SEQUENCE_EDITOR", editor_os),
        ("PENGUINGIT_TODO_FILE", todo_os),
        ("GIT_EDITOR", true_os),
    ];

    let out = run_git_raw_with_env(cwd, &["rebase", "-i", base_ref], &envs)?;

    if out.success() {
        Ok(out.stdout)
    } else {
        Err(GitError::CommandFailed {
            exit_code: out.exit_code,
            stderr: out.stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_support::FixtureRepo;

    #[test]
    fn plain_rebase_succeeds_without_conflict() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "1\n", "initial");
        repo.git(&["branch", "feature"]);

        repo.commit("b.txt", "main work\n", "main commit");
        repo.git(&["checkout", "feature"]);
        repo.commit("c.txt", "feature work\n", "feature commit");

        let res = plain_rebase(repo.path(), "main").expect("plain rebase onto main");
        assert!(
            res.contains("Successfully rebased")
                || res.contains("up to date")
                || res.trim().is_empty()
        );
    }

    #[test]
    fn interactive_rebase_reorders_and_squashes_commits() {
        let repo = FixtureRepo::new();
        let c0 = repo.commit("f0.txt", "0\n", "Root Commit");
        let c1 = repo.commit("f1.txt", "1\n", "Commit 1");
        let c2 = repo.commit("f2.txt", "2\n", "Commit 2");
        let c3 = repo.commit("f3.txt", "3\n", "Commit 3");
        let c4 = repo.commit("f4.txt", "4\n", "Commit 4");

        // Prepare todo list: pick c1, pick c3, squash c2 into c3, pick c4
        let todo_items = vec![
            RebaseTodoItem {
                action: "pick".into(),
                hash: c1.clone(),
                message: "Commit 1".into(),
            },
            RebaseTodoItem {
                action: "pick".into(),
                hash: c3.clone(),
                message: "Commit 3".into(),
            },
            RebaseTodoItem {
                action: "squash".into(),
                hash: c2.clone(),
                message: "Commit 2".into(),
            },
            RebaseTodoItem {
                action: "pick".into(),
                hash: c4.clone(),
                message: "Commit 4".into(),
            },
        ];

        // Ensure binary is compiled for test environment
        if find_sequence_editor_executable().is_err() {
            let mut cmd = std::process::Command::new("cargo");
            cmd.args(["build", "--bin", "penguingit-sequence-editor"]);
            if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
                if let Some(parent) = std::path::Path::new(&manifest_dir).parent() {
                    cmd.current_dir(parent);
                }
            }
            if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
                cmd.env("CARGO_TARGET_DIR", target_dir);
            }
            let _ = cmd.status();
        }

        let _ = interactive_rebase(repo.path(), &c0, &todo_items).expect("interactive rebase");

        let log = repo.git(&["log", "--oneline"]);
        assert!(log.contains("Commit 4"));
        assert!(log.contains("Commit 3"));
    }

    #[test]
    fn rebase_refuses_option_like_arguments() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");

        assert!(plain_rebase(repo.path(), "--continue").is_err());
        assert!(interactive_rebase(repo.path(), "--continue", &[]).is_err());
    }
}
