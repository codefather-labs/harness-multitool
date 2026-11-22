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

/// Where a folder's journal is (CON-197): its file and the key it is kept under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub file: PathBuf,
    pub key: PathBuf,
}

/// The place of a folder's journal, from what is known of it: in a git work tree, the data dir's
/// file for its key (its remote's, or its path); elsewhere `<home>/projects/<name>-<hex>/journal.db`,
/// named as the data dir's file would be, so one folder is always one place.
pub fn place_for(data_dir: &Path, home: &Path, root: &Path, remote: Option<&str>, in_work_tree: bool) -> Place {
    let key = key_for(remote, root);
    if in_work_tree {
        return Place { file: crate::journal::journal_path(&data_dir.join("journals"), &key), key };
    }
    let name = crate::journal::journal_file_name(&key);
    let folder = name.strip_suffix(".db").unwrap_or(&name).to_string();
    Place { file: home.join("projects").join(folder).join("journal.db"), key }
}

/// Where the harness keeps its own files: MULTITOOL_HOME, else `~/.harness`.
pub fn harness_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("MULTITOOL_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".harness")
}

/// The home whose `projects/` holds the journals of folders with no repository. The harness's own
/// data dir has the user's home (MULTITOOL_HOME, else `~/.harness`); another data dir is a harness
/// of its own, a test's or a debug run's, and keeps them in itself, so it never writes to the
/// user's. MULTITOOL_HOME set names it in every case.
pub fn home_for(data_dir: &Path) -> PathBuf {
    let standard = dirs::data_dir().map(|d| d.join("harness-multitool"));
    if std::env::var_os("MULTITOOL_HOME").is_some_and(|d| !d.is_empty()) || standard.as_deref() == Some(data_dir) {
        harness_home()
    } else {
        data_dir.join("home")
    }
}

/// Whether `root` is inside a git work tree.
pub fn in_work_tree(root: &Path) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
}

/// The place of `root`'s journal, as git and the environment say (CON-197). A folder that is no
/// work tree and still has its journal in the data dir from before has it moved to its place once.
pub fn place(data_dir: &Path, root: &Path, remote: Option<&str>) -> Place {
    let inside = in_work_tree(root);
    let place = place_for(data_dir, &home_for(data_dir), root, remote, inside);
    if !inside {
        let old = crate::journal::journal_path(&data_dir.join("journals"), &place.key);
        let _ = move_journal(&old, &place.file);
    }
    place
}

/// A journal file and its WAL companions moved to `to`, when `to` has none yet: renamed, or
/// copied and removed where the two are on different file systems.
fn move_journal(from: &Path, to: &Path) -> std::io::Result<bool> {
    if !from.exists() || to.exists() {
        return Ok(false);
    }
    if let Ok(conn) = rusqlite::Connection::open(from) {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::rename(from, to).is_err() {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    for ext in ["db-wal", "db-shm"] {
        let _ = std::fs::remove_file(from.with_extension(ext));
    }
    Ok(true)
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

    /// CON-197: a folder that is no repository keeps its journal under the harness's home.
    #[test]
    fn a_folder_with_no_repository_keeps_its_journal_in_the_harness_home() {
        let (data, home) = (Path::new("/data"), Path::new("/home/u/.harness"));
        let outside = place_for(data, home, Path::new("/home/u/notes"), None, false);
        assert_eq!(outside.key, PathBuf::from("/home/u/notes"));
        assert!(outside.file.starts_with("/home/u/.harness/projects") && outside.file.ends_with("journal.db"), "{:?}", outside.file);
        let folder = outside.file.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        assert!(folder.starts_with("notes-") && folder.len() == "notes-".len() + 12, "{folder}");
        let repo = place_for(data, home, Path::new("/w/a"), Some("git@github.com:foo/bar.git"), true);
        assert_eq!((repo.key.clone(), repo.file.starts_with("/data/journals")), (PathBuf::from("git:github.com/foo/bar"), true));
        let local_repo = place_for(data, home, Path::new("/w/b"), None, true);
        assert!(local_repo.file.starts_with("/data/journals"), "a repository with no remote stays in the data dir");
    }

    /// CON-197: a non-repo folder's journal from before is moved to its place once, its entries kept.
    #[test]
    fn an_old_journal_moves_to_its_place_once() {
        let tmp = std::env::temp_dir().join(format!("mt-place-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let (data, home, root) = (tmp.join("data"), tmp.join("home"), tmp.join("notes"));
        std::fs::create_dir_all(&root).unwrap();
        let key = key_for(None, &root);
        let mut conn = crate::journal::open(&data.join("journals"), &key, true).unwrap().unwrap();
        crate::journal::add(&mut conn, &crate::journal::NewEntry { body: "kept".into(), category: "note".into(), tags: vec!["t".into()], ..Default::default() }, 50).unwrap();
        drop(conn);
        let to = place_for(&data, &home, &root, None, false);
        assert!(move_journal(&crate::journal::journal_path(&data.join("journals"), &key), &to.file).unwrap());
        assert!(!crate::journal::journal_path(&data.join("journals"), &key).exists(), "moved, not copied");
        let conn = crate::journal::open_at(&to.file, &to.key, false).unwrap().unwrap();
        assert_eq!(crate::journal::list(&conn, &crate::journal::Filter::default(), 0, 10).unwrap().0, 1);
        assert!(!move_journal(&crate::journal::journal_path(&data.join("journals"), &key), &to.file).unwrap(), "once");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_folder_without_a_remote_keys_by_its_path() {
        assert_eq!(key_for(None, Path::new("/work/a")), PathBuf::from("/work/a"));
        assert_eq!(key_for(Some("git@github.com:foo/bar.git"), Path::new("/work/a")), PathBuf::from("git:github.com/foo/bar"));
    }
}
