//! Which journal a folder uses (CON-22): one per repository, identified by its remote, so every
//! checkout shares it; a folder with no remote has its own.

use std::path::{Path, PathBuf};

/// A remote URL as `host/owner/repo`: the same for its SSH and HTTPS forms.
pub fn normalize_remote_url(url: &str) -> Option<String> {
    let lower = url.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }
    let s = if let Some(rest) = lower.strip_prefix("git@") {
        rest.replacen(':', "/", 1)
    } else if let Some(rest) = ["ssh://git@", "ssh://", "https://", "http://", "git://"].iter().find_map(|p| lower.strip_prefix(p)) {
        rest.to_string()
    } else {
        lower.clone()
    };
    let s = s.trim_end_matches('/').trim_end_matches(".git");
    (s.contains('/') && !s.starts_with('/')).then(|| s.to_string())
}

/// The folder's journal key, given its `origin` URL if it has one.
pub fn key_for(remote: Option<&str>, folder: &Path) -> PathBuf {
    crate::journal::project_key(remote.and_then(normalize_remote_url).as_deref(), folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_of_a_remote_is_one_project() {
        for url in ["git@github.com:Foo/Bar.git", "https://github.com/foo/bar.git", "ssh://git@github.com/foo/bar", "https://github.com/foo/bar/"] {
            assert_eq!(normalize_remote_url(url).as_deref(), Some("github.com/foo/bar"), "{url}");
        }
        assert_eq!(normalize_remote_url(""), None);
        assert_eq!(normalize_remote_url("/local/path"), None);
    }

    #[test]
    fn a_folder_without_a_remote_keys_by_its_path() {
        assert_eq!(key_for(None, Path::new("/work/a")), PathBuf::from("/work/a"));
        assert_eq!(key_for(Some("git@github.com:foo/bar.git"), Path::new("/work/a")), PathBuf::from("git:github.com/foo/bar"));
    }
}
