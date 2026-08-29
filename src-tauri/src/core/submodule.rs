use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::branch::reject_option_like;
use super::exec::{run_git, GitError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmoduleStatus {
    pub path: String,
    pub sha: String,
    pub initialized: bool,
    pub has_changes: bool,
    pub name: Option<String>,
    pub url: Option<String>,
}

/// Metadata parsed from `.gitmodules`.
#[derive(Debug, Default, Clone)]
pub struct SubmoduleMeta {
    pub name: String,
    pub path: String,
    pub url: String,
}

/// Parses `.gitmodules` content into a map of `submodule_path` -> `SubmoduleMeta`.
pub fn parse_gitmodules(content: &str) -> HashMap<String, SubmoduleMeta> {
    let mut map = HashMap::new();
    let mut current_name: Option<String> = None;
    let mut current_path: Option<String> = None;
    let mut current_url: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            // Save previous section if complete
            if let (Some(name), Some(path), Some(url)) =
                (current_name.take(), current_path.take(), current_url.take())
            {
                map.insert(path.clone(), SubmoduleMeta { name, path, url });
            }
            // Parse section header [submodule "name"]
            let header = &trimmed[1..trimmed.len() - 1];
            if header.starts_with("submodule ") {
                let name_part = header.trim_start_matches("submodule ").trim();
                let name = name_part.trim_matches('"');
                current_name = Some(name.to_string());
            }
        } else if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if key == "path" {
                current_path = Some(value.to_string());
            } else if key == "url" {
                current_url = Some(value.to_string());
            }
        }
    }

    if let (Some(name), Some(path), Some(url)) = (current_name, current_path, current_url) {
        map.insert(path.clone(), SubmoduleMeta { name, path, url });
    }

    map
}

/// Parses output of `git submodule status` line by line.
pub fn parse_submodule_status_line(
    line: &str,
    meta_map: &HashMap<String, SubmoduleMeta>,
) -> Option<SubmoduleStatus> {
    if line.is_empty() {
        return None;
    }
    let first_char = line.chars().next()?;
    let rest = &line[1..];
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let sha = parts[0].to_string();
    let path = parts[1].to_string();

    let (initialized, has_changes) = match first_char {
        '-' => (false, false),
        '+' | 'U' => (true, true),
        ' ' => (true, false),
        _ => (true, false),
    };

    let meta = meta_map.get(&path);

    Some(SubmoduleStatus {
        path,
        sha,
        initialized,
        has_changes,
        name: meta.map(|m| m.name.clone()),
        url: meta.map(|m| m.url.clone()),
    })
}

/// Lists all submodules in `repo_path` with their status and metadata.
pub fn get_submodules(repo_path: &Path) -> Result<Vec<SubmoduleStatus>, GitError> {
    let gitmodules_path = repo_path.join(".gitmodules");
    let meta_map = if gitmodules_path.exists() {
        match std::fs::read_to_string(&gitmodules_path) {
            Ok(content) => parse_gitmodules(&content),
            Err(_) => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    let raw_status = match run_git(repo_path, &["submodule", "status"]) {
        Ok(out) => out,
        Err(_) => return Ok(Vec::new()),
    };

    let mut list = Vec::new();
    for line in raw_status.lines() {
        if let Some(status) = parse_submodule_status_line(line, &meta_map) {
            list.push(status);
        }
    }
    Ok(list)
}

/// Initializes a submodule in `repo_path` (`git submodule init <path>`).
pub fn init_submodule(repo_path: &Path, submodule_path: &str) -> Result<(), GitError> {
    reject_option_like(submodule_path)?;
    run_git(repo_path, &["submodule", "init", "--", submodule_path])?;
    Ok(())
}

/// Updates a submodule in `repo_path` (`git submodule update <path>`).
pub fn update_submodule(repo_path: &Path, submodule_path: &str) -> Result<(), GitError> {
    reject_option_like(submodule_path)?;
    run_git(repo_path, &["submodule", "update", "--", submodule_path])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gitmodules() {
        let content = r#"
[submodule "libs/foo"]
	path = libs/foo
	url = https://github.com/example/foo.git
[submodule "plugins/bar"]
	path = plugins/bar
	url = git@github.com:example/bar.git
"#;
        let map = parse_gitmodules(content);
        assert_eq!(map.len(), 2);

        let foo = map.get("libs/foo").unwrap();
        assert_eq!(foo.name, "libs/foo");
        assert_eq!(foo.path, "libs/foo");
        assert_eq!(foo.url, "https://github.com/example/foo.git");

        let bar = map.get("plugins/bar").unwrap();
        assert_eq!(bar.name, "plugins/bar");
        assert_eq!(bar.url, "git@github.com:example/bar.git");
    }

    #[test]
    fn test_parse_submodule_status_lines() {
        let meta_map = HashMap::new();

        // Initialized, clean
        let line1 = " e1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0 vendor/lib (v1.0)";
        let s1 = parse_submodule_status_line(line1, &meta_map).unwrap();
        assert_eq!(s1.path, "vendor/lib");
        assert_eq!(s1.sha, "e1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0");
        assert!(s1.initialized);
        assert!(!s1.has_changes);

        // Uninitialized (-)
        let line2 = "-1234567890abcdef1234567890abcdef12345678 vendor/uninit";
        let s2 = parse_submodule_status_line(line2, &meta_map).unwrap();
        assert_eq!(s2.path, "vendor/uninit");
        assert!(!s2.initialized);
        assert!(!s2.has_changes);

        // Has changes (+)
        let line3 = "+abcdef1234567890abcdef1234567890abcdef12 vendor/modified (heads/main)";
        let s3 = parse_submodule_status_line(line3, &meta_map).unwrap();
        assert_eq!(s3.path, "vendor/modified");
        assert!(s3.initialized);
        assert!(s3.has_changes);

        // Conflict (U)
        let line4 = "U9876543210fedcba9876543210fedcba98765432 vendor/conflict";
        let s4 = parse_submodule_status_line(line4, &meta_map).unwrap();
        assert_eq!(s4.path, "vendor/conflict");
        assert!(s4.initialized);
        assert!(s4.has_changes);
    }

    #[test]
    fn test_init_submodule_rejects_option_like() {
        let repo_path = Path::new(".");
        let res = init_submodule(repo_path, "-invalid");
        assert!(res.is_err());
        
        let res = update_submodule(repo_path, "-invalid");
        assert!(res.is_err());
    }
}
