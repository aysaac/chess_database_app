//! Persistent user settings (INF-02): configured engines, recent databases
//! (DB-07) and the frontend's own preferences, stored as JSON in the app's
//! config directory. `CHESSDB_CONFIG_DIR` overrides the directory, so test
//! runs don't touch the user's settings.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "settings.json";
const MAX_RECENT: usize = 12;

/// A configured UCI engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineConfig {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Found automatically (bundled or on PATH) rather than added by hand.
    #[serde(default)]
    pub detected: bool,
    /// Options the engine declares, cached from its last start.
    #[serde(default)]
    pub declared: Vec<chess_engine::UciOption>,
    /// Option values the user changed from the defaults.
    #[serde(default)]
    pub options: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub engines: Vec<EngineConfig>,
    /// Engine used for analysis and the blunder check.
    pub default_engine: Option<String>,
    /// Database folders, most recently opened first.
    pub recent_databases: Vec<PathBuf>,
    /// The reference database (DB-13) the opening tree can use.
    pub reference_database: Option<PathBuf>,
    /// Preferences owned by the frontend (board orientation, pane sizes…).
    pub ui: serde_json::Map<String, serde_json::Value>,
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("ignoring unreadable settings {}: {e}", path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    /// Writes the settings atomically (temp file + rename).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, dir.join(FILE))
    }

    pub fn engine(&self, id: &str) -> Option<&EngineConfig> {
        self.engines.iter().find(|e| e.id == id)
    }

    pub fn engine_mut(&mut self, id: &str) -> Option<&mut EngineConfig> {
        self.engines.iter_mut().find(|e| e.id == id)
    }

    /// The default engine, or the first one.
    pub fn preferred_engine(&self) -> Option<&EngineConfig> {
        self.default_engine.as_deref().and_then(|id| self.engine(id)).or_else(|| self.engines.first())
    }

    pub fn note_recent(&mut self, path: &Path) {
        self.recent_databases.retain(|p| p != path);
        self.recent_databases.insert(0, path.to_path_buf());
        self.recent_databases.truncate(MAX_RECENT);
    }
}

/// A short unique id for a new engine entry.
pub fn new_id(taken: &[EngineConfig]) -> String {
    (1..).map(|n| format!("engine-{n}")).find(|id| taken.iter().all(|e| &e.id != id)).expect("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.engines.push(EngineConfig {
            id: new_id(&s.engines),
            name: "Stockfish".into(),
            path: "/x/stockfish".into(),
            detected: true,
            declared: vec![chess_engine::UciOption::Check { name: "Ponder".into(), default: false }],
            options: vec![("Threads".into(), "4".into())],
        });
        s.note_recent(Path::new("/a.chessdb"));
        s.note_recent(Path::new("/b.chessdb"));
        s.note_recent(Path::new("/a.chessdb"));
        s.ui.insert("orientation".into(), "black".into());
        s.save(dir.path()).unwrap();
        let loaded = Settings::load(dir.path());
        assert_eq!(loaded, s);
        assert_eq!(loaded.recent_databases, [PathBuf::from("/a.chessdb"), PathBuf::from("/b.chessdb")]);
        assert_eq!(new_id(&loaded.engines), "engine-2");

        std::fs::write(dir.path().join(FILE), b"{ not json").unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
        // Unknown and missing fields are fine.
        std::fs::write(dir.path().join(FILE), br#"{"recentDatabases": ["/c"], "future": 1}"#).unwrap();
        assert_eq!(Settings::load(dir.path()).recent_databases, [PathBuf::from("/c")]);
    }
}
