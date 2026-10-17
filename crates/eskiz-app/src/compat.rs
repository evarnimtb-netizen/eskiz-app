//! Compatibility with releases that used the old app name: environment variables, data folders
//! and eframe storage keys. Everything here reads the old name and writes the new one. Lines
//! that carry the old name are tagged `legacy-easel` so the rename script leaves them alone.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Prefix of the environment variables of older releases.
const LEGACY_ENV_PREFIX: &str = "EASEL_"; // legacy-easel
const ENV_PREFIX: &str = "ESKIZ_";

/// Looks `name` (an `ESKIZ_*` variable) up through `get`, then the old `ESKIZ_*` spelling.
pub(crate) fn lookup_env(name: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    get(name).or_else(|| {
        let rest = name.strip_prefix(ENV_PREFIX)?;
        get(&format!("{LEGACY_ENV_PREFIX}{rest}"))
    })
}

/// The value of the `ESKIZ_*` variable `name`, or of its old `ESKIZ_*` spelling.
pub(crate) fn env_os(name: &str) -> Option<OsString> {
    lookup_env(name, |n| std::env::var_os(n))
}

/// As [`env_os`], as a string; `None` when unset or not Unicode.
pub(crate) fn env_var(name: &str) -> Option<String> {
    env_os(name).and_then(|v| v.into_string().ok())
}

/// The folder to use for `new`, moving the old folder `legacy` there first when only the old one
/// exists. When the move fails the old folder keeps being used, so nothing is lost.
pub(crate) fn migrate_dir(new: PathBuf, legacy: &Path) -> PathBuf {
    if new.exists() || !legacy.exists() {
        return new;
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(legacy, &new) {
        Ok(()) => new,
        Err(_) => legacy.to_owned(),
    }
}

/// The per-user data folder under the old name (`Eskiz`), if the platform has one.
pub(crate) fn legacy_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Easel").map(|d| d.data_dir().to_owned()) // legacy-easel
}

/// The old folder name in the home directory, for hosts without a platform data folder.
pub(crate) const LEGACY_HOME_DIR: &str = "Easel"; // legacy-easel

/// Reads an eframe storage entry under `key`, or under `legacy_key` when `key` is missing.
pub(crate) fn storage_string(storage: &dyn eframe::Storage, key: &str, legacy_key: &str) -> Option<String> {
    storage.get_string(key).or_else(|| storage.get_string(legacy_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::Storage as _;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| ((*k).to_owned(), OsString::from(v))).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn old_env_names_still_work_and_new_ones_win() {
        let old = env(&[("EASEL_RENDERER", "cpu")]); // legacy-easel
        assert_eq!(lookup_env("ESKIZ_RENDERER", old), Some("cpu".into()));
        let both = env(&[("EASEL_RENDERER", "cpu"), ("ESKIZ_RENDERER", "gl")]); // legacy-easel
        assert_eq!(lookup_env("ESKIZ_RENDERER", both), Some("gl".into()));
        assert_eq!(lookup_env("ESKIZ_RENDERER", env(&[])), None);
        assert_eq!(lookup_env("HOME", env(&[])), None, "only ESKIZ_ names have an old spelling");
    }

    #[test]
    fn the_old_data_folder_is_moved_once() {
        let root = std::env::temp_dir().join(format!("eskiz-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (old, new) = (root.join("Easel"), root.join("Eskiz")); // legacy-easel
        std::fs::create_dir_all(old.join("Documents")).unwrap();
        std::fs::write(old.join("Documents").join("a.procreate"), b"x").unwrap();
        assert_eq!(migrate_dir(new.clone(), &old), new);
        assert!(new.join("Documents").join("a.procreate").exists());
        assert!(!old.exists());
        // Already migrated, and nothing old: the new folder is returned untouched.
        assert_eq!(migrate_dir(new.clone(), &old), new);
        assert!(new.join("Documents").join("a.procreate").exists());
        // Both exist: the new one wins and the old one is left alone.
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(migrate_dir(new.clone(), &old), new);
        assert!(old.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_to_migrate_is_a_noop() {
        let root = std::env::temp_dir().join(format!("eskiz-compat-none-{}", std::process::id()));
        let new = root.join("Eskiz");
        assert_eq!(migrate_dir(new.clone(), &root.join("Easel")), new); // legacy-easel
        assert!(!new.exists());
    }

    #[derive(Default)]
    struct Mem(HashMap<String, String>);
    impl eframe::Storage for Mem {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn preferences_saved_under_the_old_keys_are_read_and_new_keys_win() {
        use crate::app::{Prefs, LEGACY_PREF_KEY, PREF_KEY};
        let mut old = Mem::default();
        old.set_string(LEGACY_PREF_KEY, Prefs { light: true, ..Prefs::default() }.encode());
        let text = storage_string(&old, PREF_KEY, LEGACY_PREF_KEY).expect("old key is read");
        assert!(Prefs::decode(&text).light);
        let mut both = Mem::default();
        both.set_string(LEGACY_PREF_KEY, Prefs { light: true, ..Prefs::default() }.encode());
        both.set_string(PREF_KEY, Prefs { light: false, ..Prefs::default() }.encode());
        let text = storage_string(&both, PREF_KEY, LEGACY_PREF_KEY).unwrap();
        assert!(!Prefs::decode(&text).light);
        assert_eq!(storage_string(&Mem::default(), PREF_KEY, LEGACY_PREF_KEY), None);
    }
}
