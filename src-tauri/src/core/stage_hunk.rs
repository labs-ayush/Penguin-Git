use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;

use super::exec::{run_git, GitError};

/// Applies a unified-diff patch to the git index (`git apply --cached`).
///
/// This allows hunk-level and line-level staging by constructing a valid patch
/// for selected hunks/lines and applying it directly to the staging area.
pub fn git_stage_hunk(repo_path: &Path, patch: &str) -> Result<(), GitError> {
    if patch.trim().is_empty() {
        return Ok(());
    }

    validate_patch(patch)?;

    let mut temp_file = NamedTempFile::new().map_err(GitError::Spawn)?;
    temp_file
        .write_all(patch.as_bytes())
        .map_err(GitError::Spawn)?;
    temp_file.flush().map_err(GitError::Spawn)?;

    let temp_path = temp_file
        .path()
        .to_str()
        .ok_or_else(|| GitError::CommandFailed {
            exit_code: None,
            stderr: "Invalid temp file path".to_string(),
        })?;

    // Apply the patch to the index
    run_git(repo_path, &["apply", "--cached", temp_path])?;
    Ok(())
}

fn validate_patch(patch: &str) -> Result<(), GitError> {
    for line in patch.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("diff --git ") {
            let paths = parse_diff_git_line(trimmed);
            for p in paths {
                validate_single_path(&p)?;
            }
        } else if trimmed.starts_with("--- ") {
            if let Some(p) = parse_single_path(trimmed, "--- ") {
                validate_single_path(&p)?;
            }
        } else if trimmed.starts_with("+++ ") {
            if let Some(p) = parse_single_path(trimmed, "+++ ") {
                validate_single_path(&p)?;
            }
        } else if trimmed.starts_with("rename from ") {
            if let Some(p) = parse_single_path(trimmed, "rename from ") {
                validate_single_path(&p)?;
            }
        } else if trimmed.starts_with("rename to ") {
            if let Some(p) = parse_single_path(trimmed, "rename to ") {
                validate_single_path(&p)?;
            }
        } else if trimmed.starts_with("copy from ") {
            if let Some(p) = parse_single_path(trimmed, "copy from ") {
                validate_single_path(&p)?;
            }
        } else if trimmed.starts_with("copy to ") {
            if let Some(p) = parse_single_path(trimmed, "copy to ") {
                validate_single_path(&p)?;
            }
        }
    }
    Ok(())
}

fn parse_diff_git_line(line: &str) -> Vec<String> {
    let content = &line["diff --git ".len()..];
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;

    for c in content.chars() {
        if escaped {
            current.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            in_quotes = !in_quotes;
        } else if c == ' ' && !in_quotes {
            if !current.is_empty() {
                paths.push(current);
                current = String::new();
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        paths.push(current);
    }
    paths
}

fn parse_single_path(line: &str, prefix: &str) -> Option<String> {
    let mut content = line.strip_prefix(prefix)?.trim();
    if content.starts_with('"') {
        let mut path = String::new();
        let mut escaped = false;
        for c in content.chars().skip(1) {
            if escaped {
                path.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                return Some(path);
            } else {
                path.push(c);
            }
        }
        Some(path)
    } else {
        if let Some(tab_idx) = content.find('\t') {
            content = &content[..tab_idx];
        }
        Some(content.trim().to_string())
    }
}

fn validate_single_path(path: &str) -> Result<(), GitError> {
    if path == "/dev/null" || path == "dev/null" {
        return Ok(());
    }

    check_path_safety(path)?;
    check_path_safety(strip_git_prefix(path))?;

    Ok(())
}

fn check_path_safety(path: &str) -> Result<(), GitError> {
    if path.is_empty() {
        return Ok(());
    }

    if path.starts_with('/') || path.starts_with('\\') {
        return Err(GitError::ValidationError(format!(
            "Absolute path references not allowed: {}",
            path
        )));
    }

    let mut chars = path.chars();
    if let (Some(c1), Some(c2)) = (chars.next(), chars.next()) {
        if c1.is_ascii_alphabetic() && c2 == ':' {
            return Err(GitError::ValidationError(format!(
                "Absolute Windows path references not allowed: {}",
                path
            )));
        }
    }

    for component in path.split(['/', '\\']) {
        if component == ".." {
            return Err(GitError::ValidationError(format!(
                "Directory traversal references ('..') not allowed: {}",
                path
            )));
        }
    }

    Ok(())
}

fn strip_git_prefix(path: &str) -> &str {
    let mut chars = path.chars();
    if let (Some(c1), Some(c2)) = (chars.next(), chars.next()) {
        if c1.is_ascii_alphabetic() && (c2 == '/' || c2 == '\\') {
            return &path[2..];
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::diff::diff_repo;
    use crate::core::test_support::FixtureRepo;

    #[test]
    fn git_stage_hunk_stages_only_the_selected_hunk() {
        let repo = FixtureRepo::new();
        let file_name = "test.txt";
        let initial_content =
            "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10\n";
        repo.commit(file_name, initial_content, "Initial commit");

        // Modify lines at the top and bottom to create two distinct hunks
        let modified_content = "Line 1 CHANGED\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10 CHANGED\n";
        repo.write(file_name, modified_content);

        // Verify working tree has both changes
        let full_diff = diff_repo(repo.path(), false).expect("diff working tree");
        assert!(full_diff.contains("Line 1 CHANGED"));
        assert!(full_diff.contains("Line 10 CHANGED"));

        // Construct a valid patch for ONLY the first hunk
        let patch = [
            format!("diff --git a/{file_name} b/{file_name}"),
            format!("--- a/{file_name}"),
            format!("+++ b/{file_name}"),
            "@@ -1,3 +1,3 @@".to_string(),
            "-Line 1".to_string(),
            "+Line 1 CHANGED".to_string(),
            " Line 2".to_string(),
            " Line 3".to_string(),
            "".to_string(),
        ]
        .join("\n");

        git_stage_hunk(repo.path(), &patch).expect("stage hunk");

        // Check index diff (`git diff --cached`)
        let staged_diff = diff_repo(repo.path(), true).expect("staged diff");

        // Staged diff MUST contain the first hunk change and MUST NOT contain the second hunk change
        assert!(
            staged_diff.contains("Line 1 CHANGED"),
            "staged diff must contain Line 1 CHANGED, got: {staged_diff}"
        );
        assert!(
            !staged_diff.contains("Line 10 CHANGED"),
            "staged diff must NOT contain Line 10 CHANGED, got: {staged_diff}"
        );
    }

    #[test]
    fn test_git_stage_hunk_path_traversal_validation() {
        let repo = FixtureRepo::new();

        // 1. Path traversal via ".."
        let patch_traversal = [
            "diff --git a/test.txt b/../test.txt",
            "--- a/test.txt",
            "+++ b/../test.txt",
            "@@ -1,1 +1,1 @@",
            "-Line 1",
            "+Line 1 CHANGED",
        ]
        .join("\n");
        let result = git_stage_hunk(repo.path(), &patch_traversal);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Directory traversal references"));

        // 2. Absolute path traversal
        let patch_absolute = [
            "diff --git a/test.txt b//etc/passwd",
            "--- a/test.txt",
            "+++ b//etc/passwd",
            "@@ -1,1 +1,1 @@",
            "-Line 1",
            "+Line 1 CHANGED",
        ]
        .join("\n");
        let result = git_stage_hunk(repo.path(), &patch_absolute);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Absolute path references"));

        // 3. Windows absolute path
        let patch_windows = [
            "diff --git \"a/test.txt\" \"b/C:\\windows\\win.ini\"",
            "--- a/test.txt",
            "+++ b/C:\\windows\\win.ini",
            "@@ -1,1 +1,1 @@",
            "-Line 1",
            "+Line 1 CHANGED",
        ]
        .join("\n");
        let result = git_stage_hunk(repo.path(), &patch_windows);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Windows path references"));
    }
}
