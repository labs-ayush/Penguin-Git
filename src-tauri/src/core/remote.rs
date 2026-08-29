use std::path::Path;

use serde::{Deserialize, Serialize};

use super::branch::reject_option_like;
use super::exec::{run_git, GitError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

/// Lists configured remotes with their fetch and push URLs.
///
/// `remote -v` emits two lines per remote (`(fetch)` and `(push)`), which are
/// folded back together here so a remote with a separate pushurl is represented
/// once rather than twice.
pub fn list_remotes(repo_path: &Path) -> Result<Vec<Remote>, GitError> {
    let raw = run_git(repo_path, &["remote", "-v"])?;
    let mut remotes: Vec<Remote> = Vec::new();

    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        // "<name>\t<url> (fetch|push)". The URL can contain spaces — a local-path
        // remote like `/mnt/My Drive/repo.git` is perfectly legal — so the only
        // safe split points are the tab after the name and the last space before
        // the trailing marker. Splitting on whitespace truncates such URLs, and
        // the truncated value then round-trips back out through `set_remote_url`.
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let Some((url, kind)) = rest.rsplit_once(' ') else {
            continue;
        };

        match remotes.iter_mut().find(|r| r.name == name) {
            Some(existing) => {
                if kind == "(push)" {
                    existing.push_url = url.to_string();
                }
            }
            None => remotes.push(Remote {
                name: name.to_string(),
                fetch_url: url.to_string(),
                push_url: url.to_string(),
            }),
        }
    }

    Ok(remotes)
}

pub fn add_remote(repo_path: &Path, name: &str, url: &str) -> Result<(), GitError> {
    reject_option_like(name)?;
    run_git(repo_path, &["remote", "add", name, url])?;
    Ok(())
}

pub fn remove_remote(repo_path: &Path, name: &str) -> Result<(), GitError> {
    reject_option_like(name)?;
    run_git(repo_path, &["remote", "remove", name])?;
    Ok(())
}

pub fn rename_remote(repo_path: &Path, old: &str, new: &str) -> Result<(), GitError> {
    reject_option_like(old)?;
    reject_option_like(new)?;
    run_git(repo_path, &["remote", "rename", old, new])?;
    Ok(())
}

pub fn set_remote_url(repo_path: &Path, name: &str, url: &str) -> Result<(), GitError> {
    reject_option_like(name)?;
    run_git(repo_path, &["remote", "set-url", name, url])?;
    Ok(())
}

/// Fetches from `remote`, or from all remotes when none is named.
///
/// `--prune` drops remote-tracking refs whose upstream branch was deleted,
/// which otherwise linger in the branch list forever.
pub fn fetch(repo_path: &Path, remote: Option<&str>) -> Result<(), GitError> {
    match remote {
        Some(remote) => {
            reject_option_like(remote)?;
            run_git(repo_path, &["fetch", "--prune", remote])?
        }
        None => run_git(repo_path, &["fetch", "--prune", "--all"])?,
    };
    Ok(())
}

pub fn pull(repo_path: &Path) -> Result<(), GitError> {
    run_git(repo_path, &["pull", "--no-edit"])?;
    Ok(())
}

/// Pushes the current branch.
///
/// `set_upstream` sends `-u` for a branch that has no upstream yet — pushing
/// without it would fail on the first push of a new branch.
pub fn push(
    repo_path: &Path,
    remote: Option<&str>,
    branch: Option<&str>,
    set_upstream: bool,
) -> Result<(), GitError> {
    if let Some(remote) = remote {
        reject_option_like(remote)?;
    }
    if let Some(branch) = branch {
        reject_option_like(branch)?;
    }

    let mut args = vec!["push"];
    if set_upstream {
        args.push("-u");
    }
    if let Some(remote) = remote {
        args.push(remote);
        if let Some(branch) = branch {
            args.push(branch);
        }
    }
    run_git(repo_path, &args)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoOrigin {
    pub owner: String,
    pub repo: String,
}

/// Parses a remote URL (SSH or HTTPS) and extracts owner and repository name.
/// Handles:
/// - `git@github.com:owner/repo.git`
/// - `ssh://git@github.com/owner/repo.git`
/// - `https://github.com/owner/repo.git`
/// - `https://github.com/owner/repo`
/// - `https://user:token@github.com/owner/repo.git`
pub fn parse_remote_url(url: &str) -> Option<RepoOrigin> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cleaned = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let cleaned = cleaned.strip_suffix(".git").unwrap_or(cleaned);

    let path_part = if cleaned.contains("://") {
        let pos = cleaned.find("://")?;
        let after_proto = &cleaned[pos + 3..];
        let slash_pos = after_proto.find('/')?;
        &after_proto[slash_pos + 1..]
    } else if let Some((_host, path)) = cleaned.rsplit_once(':') {
        path.trim_start_matches('/')
    } else {
        cleaned
    };

    let parts: Vec<&str> = path_part.split('/').filter(|p| !p.is_empty()).collect();

    if parts.len() >= 2 {
        let owner = parts[parts.len() - 2].to_string();
        let repo = parts[parts.len() - 1].to_string();
        if !owner.is_empty() && !repo.is_empty() {
            return Some(RepoOrigin { owner, repo });
        }
    }

    None
}

/// Gets the `origin` remote URL for the repo at `repo_path` and parses its `{ owner, repo }`.
pub fn get_repo_origin(repo_path: &Path) -> Result<RepoOrigin, GitError> {
    let url = run_git(repo_path, &["remote", "get-url", "origin"])?;
    parse_remote_url(&url).ok_or_else(|| GitError::CommandFailed {
        exit_code: None,
        stderr: format!(
            "Failed to parse owner and repo from remote URL: {}",
            url.trim()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_support::{git_in, FixtureRepo};

    #[test]
    fn lists_a_remote_once_not_twice() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        let _bare = repo.add_bare_remote("origin");

        let remotes = list_remotes(repo.path()).expect("list");

        assert_eq!(
            remotes.len(),
            1,
            "fetch and push lines must fold into one entry"
        );
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].fetch_url, remotes[0].push_url);
    }

    #[test]
    fn remote_url_containing_a_space_survives() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        // A local-path remote with a space is legal and not unusual on a mounted
        // drive; whitespace-splitting the `remote -v` output truncates it.
        let url = "/mnt/My Drive/backup repo.git";
        add_remote(repo.path(), "spaced", url).expect("add");

        let remotes = list_remotes(repo.path()).expect("list");

        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].fetch_url, url);
        assert_eq!(remotes[0].push_url, url);
    }

    #[test]
    fn add_rename_and_remove_a_remote() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");

        add_remote(repo.path(), "upstream", "https://example.invalid/repo.git").expect("add");
        assert_eq!(list_remotes(repo.path()).unwrap().len(), 1);

        rename_remote(repo.path(), "upstream", "canonical").expect("rename");
        assert_eq!(list_remotes(repo.path()).unwrap()[0].name, "canonical");

        set_remote_url(
            repo.path(),
            "canonical",
            "https://example.invalid/other.git",
        )
        .expect("set-url");
        assert!(list_remotes(repo.path()).unwrap()[0]
            .fetch_url
            .ends_with("other.git"));

        remove_remote(repo.path(), "canonical").expect("remove");
        assert!(list_remotes(repo.path()).unwrap().is_empty());
    }

    #[test]
    fn push_and_fetch_round_trip_against_a_real_bare_remote() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        let bare = repo.add_bare_remote("origin");

        push(repo.path(), Some("origin"), Some("main"), true).expect("push");

        // The bare repo should now hold the same commit.
        let remote_head = git_in(bare.path(), &["rev-parse", "main"]);
        assert_eq!(remote_head.trim(), repo.head());

        fetch(repo.path(), Some("origin")).expect("fetch");
    }

    #[test]
    fn a_repo_with_no_remotes_lists_nothing() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");

        assert!(list_remotes(repo.path()).expect("list").is_empty());
    }

    #[test]
    fn a_separate_push_url_is_kept_alongside_the_fetch_url() {
        // `remote -v` prints one line per direction. Folding them naively would
        // either duplicate the remote or lose whichever URL came second.
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        add_remote(
            repo.path(),
            "origin",
            "https://example.invalid/read-only.git",
        )
        .expect("add");
        repo.git(&[
            "remote",
            "set-url",
            "--push",
            "origin",
            "git@example.invalid:writable.git",
        ]);

        let remotes = list_remotes(repo.path()).expect("list");

        assert_eq!(remotes.len(), 1, "still one remote, not two");
        assert_eq!(
            remotes[0].fetch_url,
            "https://example.invalid/read-only.git"
        );
        assert_eq!(remotes[0].push_url, "git@example.invalid:writable.git");
    }

    #[test]
    fn several_remotes_are_each_listed_once() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        add_remote(repo.path(), "origin", "https://example.invalid/fork.git").expect("add");
        add_remote(
            repo.path(),
            "upstream",
            "https://example.invalid/canonical.git",
        )
        .expect("add");

        let remotes = list_remotes(repo.path()).expect("list");

        let mut names: Vec<&str> = remotes.iter().map(|r| r.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["origin", "upstream"]);
    }

    #[test]
    fn adding_a_remote_that_already_exists_is_an_error() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        add_remote(repo.path(), "origin", "https://example.invalid/one.git").expect("add");

        assert!(add_remote(repo.path(), "origin", "https://example.invalid/two.git").is_err());
        assert_eq!(
            list_remotes(repo.path()).expect("list")[0].fetch_url,
            "https://example.invalid/one.git",
            "the failed add must not have overwritten the URL"
        );
    }

    #[test]
    fn removing_an_unknown_remote_is_an_error() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");

        assert!(remove_remote(repo.path(), "nope").is_err());
        assert!(rename_remote(repo.path(), "nope", "other").is_err());
        assert!(set_remote_url(repo.path(), "nope", "https://example.invalid/x.git").is_err());
    }

    #[test]
    fn fetch_prunes_a_deleted_remote_branch() {
        // Without `--prune`, a branch deleted on the remote lingers in the local
        // branch list indefinitely and the user can never make it go away.
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        let bare = repo.add_bare_remote("origin");
        push(repo.path(), Some("origin"), Some("main"), true).expect("push main");
        repo.git(&["checkout", "-b", "temporary"]);
        repo.commit_all("Work on the temporary branch");
        push(repo.path(), Some("origin"), Some("temporary"), true).expect("push temporary");
        repo.git(&["checkout", "main"]);

        assert!(repo.git(&["branch", "-r"]).contains("origin/temporary"));

        // Delete it on the remote side, the way another clone would.
        git_in(bare.path(), &["branch", "-D", "temporary"]);

        fetch(repo.path(), Some("origin")).expect("fetch");

        assert!(
            !repo.git(&["branch", "-r"]).contains("origin/temporary"),
            "the stale remote-tracking ref should have been pruned"
        );
    }

    #[test]
    fn fetch_without_a_named_remote_covers_every_remote() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        let _origin = repo.add_bare_remote("origin");
        let _upstream = repo.add_bare_remote("upstream");
        push(repo.path(), Some("origin"), Some("main"), true).expect("push");

        fetch(repo.path(), None).expect("fetch --all must not fail on a second remote");
    }

    #[test]
    fn push_without_upstream_flag_uses_the_tracking_branch() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        let bare = repo.add_bare_remote("origin");
        push(repo.path(), Some("origin"), Some("main"), true).expect("first push sets upstream");

        repo.commit("b.txt", "y", "Second");
        // No remote, no branch, no -u: git falls back to the configured upstream.
        push(repo.path(), None, None, false).expect("second push");

        assert_eq!(
            git_in(bare.path(), &["rev-parse", "main"]).trim(),
            repo.head()
        );
    }

    #[test]
    fn pull_brings_down_a_commit_made_elsewhere() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");
        let bare = repo.add_bare_remote("origin");
        push(repo.path(), Some("origin"), Some("main"), true).expect("push");

        // A second clone stands in for "somebody else pushed".
        let other = tempfile::tempdir().expect("tempdir");
        let bare_url = bare.path().to_string_lossy().to_string();
        for args in [
            vec!["clone", bare_url.as_str(), "."],
            vec!["config", "user.name", "Other Dev"],
            vec!["config", "user.email", "other@penguingit.invalid"],
            vec!["commit", "--allow-empty", "-m", "Work from elsewhere"],
            vec!["push", "origin", "main"],
        ] {
            git_in(other.path(), &args);
        }

        pull(repo.path()).expect("pull");

        assert!(repo
            .git(&["log", "--oneline"])
            .contains("Work from elsewhere"));
    }

    #[test]
    fn parse_remote_url_supports_ssh_and_https_formats() {
        // SSH SCP format
        let ssh1 = parse_remote_url("git@github.com:Ayush442842q/PenguinGit.git").unwrap();
        assert_eq!(ssh1.owner, "Ayush442842q");
        assert_eq!(ssh1.repo, "PenguinGit");

        // SSH scheme format
        let ssh2 = parse_remote_url("ssh://git@github.com/Ayush442842q/PenguinGit.git").unwrap();
        assert_eq!(ssh2.owner, "Ayush442842q");
        assert_eq!(ssh2.repo, "PenguinGit");

        // HTTPS with .git
        let https1 = parse_remote_url("https://github.com/Ayush442842q/PenguinGit.git").unwrap();
        assert_eq!(https1.owner, "Ayush442842q");
        assert_eq!(https1.repo, "PenguinGit");

        // HTTPS without .git
        let https2 = parse_remote_url("https://github.com/Ayush442842q/PenguinGit").unwrap();
        assert_eq!(https2.owner, "Ayush442842q");
        assert_eq!(https2.repo, "PenguinGit");

        // HTTPS with credentials
        let https3 =
            parse_remote_url("https://user:token@github.com/Ayush442842q/PenguinGit.git").unwrap();
        assert_eq!(https3.owner, "Ayush442842q");
        assert_eq!(https3.repo, "PenguinGit");
    }

    #[test]
    fn get_repo_origin_fetches_and_parses_origin_remote() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");

        add_remote(
            repo.path(),
            "origin",
            "git@github.com:Ayush442842q/PenguinGit.git",
        )
        .expect("add origin");

        let origin = get_repo_origin(repo.path()).expect("get_repo_origin");
        assert_eq!(origin.owner, "Ayush442842q");
        assert_eq!(origin.repo, "PenguinGit");
    }

    #[test]
    fn remote_operations_refuse_option_like_names() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "x", "Initial commit");

        assert!(add_remote(repo.path(), "--upload-pack=x", "https://example.com/x.git").is_err());
        assert!(add_remote(repo.path(), "origin", "https://example.com/x.git").is_ok());

        assert!(rename_remote(repo.path(), "-f", "renamed").is_err());
        assert!(rename_remote(repo.path(), "origin", "--mirror").is_err());
        rename_remote(repo.path(), "origin", "upstream").expect("rename should succeed");

        assert!(set_remote_url(repo.path(), "--tags", "https://example.com/y.git").is_err());
        set_remote_url(repo.path(), "upstream", "https://example.com/y.git")
            .expect("set-url should succeed");

        assert!(remove_remote(repo.path(), "-o").is_err());
        remove_remote(repo.path(), "upstream").expect("remove should succeed");

        assert!(fetch(repo.path(), Some("--upload-pack=x")).is_err());

        assert!(push(repo.path(), Some("--upload-pack=x"), None, false).is_err());
        assert!(push(repo.path(), None, Some("--receive-pack=x"), false).is_err());
    }
}
