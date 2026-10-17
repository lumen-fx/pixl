//! Where pixl keeps what the user customizes: one JSON file per kind
//! (workspace layouts, themes, tool presets...) in pixl's folder of the user
//! data directory, beside the recent list. A store without a folder keeps
//! nothing, which is how tests and the bench run the editor.

use std::path::{Path, PathBuf};

use lumenui::lumen_core::app_paths;
use pixl_core::native::write_atomic;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::recents::APP_ID;

/// A folder customization files are read from and written to, or none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Store {
    dir: Option<PathBuf>,
}

impl Store {
    /// pixl's folder of the user data directory.
    pub fn user() -> Self {
        Self::at(app_paths::data_dir_for(APP_ID))
    }

    /// A store in `dir`.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
        }
    }

    /// A store that keeps nothing.
    pub fn in_memory() -> Self {
        Self { dir: None }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The path of the file `name` in the store, when it has a folder.
    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(name))
    }

    /// Reads `name` as `T`. A missing or unreadable file reads as `None`,
    /// so a damaged file falls back to the defaults instead of stopping the
    /// editor.
    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let bytes = std::fs::read(self.path(name)?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Writes `value` to `name`, atomically. A store without a folder
    /// writes nothing.
    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> std::result::Result<(), String> {
        let Some(path) = self.path(name) else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        write_atomic(&path, &bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_reads_back_and_a_bad_file_reads_as_none() {
        let dir = std::env::temp_dir().join(format!("pixl-store-{}", std::process::id()));
        let store = Store::at(&dir);
        store.save("a.json", &vec![1, 2, 3]).unwrap();
        assert_eq!(store.load::<Vec<i32>>("a.json"), Some(vec![1, 2, 3]));
        std::fs::write(dir.join("b.json"), b"{nope").unwrap();
        assert_eq!(store.load::<Vec<i32>>("b.json"), None);
        assert_eq!(store.load::<Vec<i32>>("missing.json"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_store_in_memory_keeps_nothing() {
        let store = Store::in_memory();
        store.save("a.json", &1).unwrap();
        assert_eq!(store.load::<i32>("a.json"), None);
    }
}
