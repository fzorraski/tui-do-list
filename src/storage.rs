//! Locating, loading and atomically saving the JSON data file.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;

use crate::app::ListSlot;
use crate::config::{Config, DEFAULT_LIST};
use crate::model::Store;

pub const ENV_OVERRIDE: &str = "TUI_DO_LIST_FILE";

/// Data file for one list name, following the same scheme as `list_paths`.
pub fn data_path_for(name: &str) -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "tui-do-list")
        .context("could not determine a data directory for this user")?;
    Ok(dirs.data_dir().join(format!("{name}.json")))
}

/// `(name, path)` for every configured list. `$TUI_DO_LIST_FILE` forces a single
/// unnamed list at that path.
pub fn list_paths(config: &Config) -> Result<Vec<(String, PathBuf)>> {
    if let Some(p) = std::env::var_os(ENV_OVERRIDE) {
        return Ok(vec![(DEFAULT_LIST.to_string(), PathBuf::from(p))]);
    }
    config
        .lists
        .iter()
        .map(|name| Ok((name.clone(), data_path_for(name)?)))
        .collect()
}

/// Loads every configured list, in config order.
pub fn load_lists(config: &Config) -> Result<Vec<ListSlot>> {
    list_paths(config)?
        .into_iter()
        .map(|(name, path)| {
            Ok(ListSlot {
                store: load(&path)?,
                name,
                path,
            })
        })
        .collect()
}

/// A missing file yields an empty store; an unreadable or corrupt file is an
/// error so we never clobber it on the next save.
pub fn load(path: &Path) -> Result<Store> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut store: Store =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    // Reconcile next_id with the tasks actually present, so a hand-edited or
    // partially restored file can never hand out duplicate ids.
    let max_id = store
        .days
        .values()
        .flatten()
        .map(|t| t.id)
        .max()
        .unwrap_or(0);
    store.next_id = store.next_id.max(max_id + 1);
    if store.version > crate::model::STORE_VERSION {
        bail!(
            "{} was written by a newer version (format v{})",
            path.display(),
            store.version
        );
    }
    Ok(store)
}

/// Writes to `<file>.tmp` then renames over the target so a crash mid-write
/// can never leave a truncated file behind.
pub fn save(path: &Path, store: &Store) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    // Process-unique temp name so concurrent writers cannot interleave.
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    let json = serde_json::to_string_pretty(store)?;
    let mut file = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(json.as_bytes())
        .with_context(|| format!("writing {}", tmp.display()))?;
    // Flush data to disk BEFORE the rename becomes visible, so a power cut
    // can never leave an empty/truncated tasks file behind.
    file.sync_all()
        .with_context(|| format!("syncing {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path)
        .with_context(|| format!("moving {} to {}", tmp.display(), path.display()))?;
    // Best-effort fsync of the directory so the rename itself is durable.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Removes its lock file when dropped.
#[derive(Debug)]
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Prevents two interactive instances from editing the same data file and
/// silently overwriting each other's saves. The lock file holds our pid; a
/// lock whose pid no longer exists (checked via /proc) is treated as stale.
pub fn acquire_lock(data_path: &Path) -> Result<LockGuard> {
    let path = data_path.with_extension("lock");
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    for attempt in 0..2 {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = file.write_all(std::process::id().to_string().as_bytes());
                return Ok(LockGuard { path });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                let pid = fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok());
                let stale = match pid {
                    Some(pid) => {
                        Path::new("/proc").exists() && !Path::new(&format!("/proc/{pid}")).exists()
                    }
                    None => false,
                };
                if !stale {
                    bail!(
                        "another tui-do-list instance{} is already editing this data — close it first, or delete {} if that instance crashed",
                        pid.map(|p| format!(" (pid {p})")).unwrap_or_default(),
                        path.display()
                    );
                }
                fs::remove_file(&path)
                    .with_context(|| format!("removing stale lock {}", path.display()))?;
            }
            Err(e) => {
                return Err(e).with_context(|| format!("creating lock {}", path.display()));
            }
        }
    }
    bail!("could not acquire lock {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tui-do-list-test-{}", std::process::id()));
        dir.join(name)
    }

    #[test]
    fn missing_file_is_an_empty_store() {
        let store = load(&temp_file("does-not-exist.json")).unwrap();
        assert!(store.days.is_empty());
        assert_eq!(store.next_id, 1);
    }

    #[test]
    fn save_then_load_round_trips_and_leaves_no_tmp() {
        let path = temp_file("roundtrip/tasks.json");
        let mut store = Store::default();
        let day = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();
        store.add(day, "café ☕");
        store.toggle(day, 0, day);
        store.cycle_priority(day, 0);

        save(&path, &store).unwrap();
        let leftovers = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("tmp")
            })
            .count();
        assert_eq!(leftovers, 0, "no temp files left behind");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.tasks(day), store.tasks(day));
        assert_eq!(loaded.next_id, store.next_id);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn load_reconciles_next_id_with_existing_tasks() {
        let path = temp_file("nextid.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"version":1,"next_id":1,"days":{"2026-08-31":[
                {"id":7,"text":"a","created":"2026-08-31"}]}}"#,
        )
        .unwrap();
        assert_eq!(load(&path).unwrap().next_id, 8);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn lock_blocks_second_instance_and_recovers_stale_locks() {
        let data = temp_file("lock/tasks.json");
        let guard = acquire_lock(&data).unwrap();
        let lock_path = data.with_extension("lock");
        assert!(lock_path.exists());
        let err = acquire_lock(&data).unwrap_err().to_string();
        assert!(err.contains("another tui-do-list instance"), "{err}");
        drop(guard);
        assert!(!lock_path.exists(), "released on drop");

        // A lock left by a dead process is taken over.
        fs::write(&lock_path, "4294967294").unwrap();
        let guard = acquire_lock(&data).unwrap();
        drop(guard);
        // Garbage contents are NOT treated as stale.
        fs::write(&lock_path, "who knows").unwrap();
        assert!(acquire_lock(&data).is_err());
        let _ = fs::remove_dir_all(data.parent().unwrap());
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let path = temp_file("corrupt.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).is_err());
        let _ = fs::remove_file(&path);
    }
}
