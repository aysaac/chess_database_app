//! Persistent user settings (INF-02): configured engines, recent databases
//! (DB-07), how each database's game list looked (QOL-07), whether the
//! welcome guide ran (QOL-08) and the frontend's own preferences, stored as
//! JSON in the app's config directory. `CHESSDB_CONFIG_DIR` overrides the
//! directory, so test runs don't touch the user's settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dto::{HeaderQueryDto, NameKindDto, OnlineSiteDto, SortSpecDto, SpeedDto};

const FILE: &str = "settings.json";
const MAX_RECENT: usize = 12;
/// Databases whose list view is remembered; the least recently used go.
const MAX_VIEWS: usize = 50;
/// Downloads remembered per database (NET-01), the most recent first.
const MAX_MARKS: usize = 20;

/// One step of a game-list filter (QOL-07): enough to make the filter again
/// when the database is reopened, since game ids don't last (compaction
/// renumbers them) and new games may match. Steps after the first searched
/// within the results of the ones before (SRCH-14).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FilterStep {
    /// A header search (SRCH-01), or the quick filter box (SRCH-02).
    #[serde(rename_all = "camelCase")]
    Header {
        query: HeaderQueryDto,
        #[serde(default)]
        quick: bool,
    },
    /// The games of one player, event, site or annotator (DB-04), by name.
    #[serde(rename_all = "camelCase")]
    Name { table: NameKindDto, name: String },
    /// The games reaching a position (SRCH-03); `full` searched every game
    /// at any depth instead of the position index.
    #[serde(rename_all = "camelCase")]
    Position {
        fen: String,
        #[serde(default)]
        full: bool,
    },
    /// A step this version doesn't know (a newer one saved it). The filter
    /// is dropped when the settings are read, and the rest of them kept:
    /// failing to read the whole file would lose them at the next save.
    #[serde(other)]
    Unknown,
}

/// A filter to make again when its database is reopened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFilter {
    pub steps: Vec<FilterStep>,
    /// As the filter bar showed it.
    pub description: String,
}

/// How the game list of one database looked (QOL-07): its sort order, the
/// column widths the user chose and its filter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DbView {
    /// The database folder (canonical, as `AppState` keeps it).
    pub path: PathBuf,
    /// `None` for the natural order (by game number).
    pub sort: Option<SortSpecDto>,
    /// Widths in CSS pixels of the columns the user resized, by column id.
    pub columns: BTreeMap<String, u32>,
    pub filter: Option<SavedFilter>,
    /// How far each download of a player's online games into it got
    /// (NET-01), for "new ones only".
    pub downloads: Vec<DownloadMark>,
}

/// How far a download of a player's games got (NET-01): every game of
/// theirs from `site` of those time controls (rated only or not) that
/// started (Lichess) or ended (Chess.com) before `reached` is in the
/// database. Kept with the database's view, so a database moved or made
/// again starts over (all games, those it has skipped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadMark {
    pub site: OnlineSiteDto,
    /// In lowercase: both sites ignore case.
    pub player: String,
    /// Sorted; empty for all of them.
    pub speeds: Vec<SpeedDto>,
    pub rated_only: bool,
    /// Ms since the epoch.
    pub reached: i64,
}

impl DownloadMark {
    pub fn new(
        site: OnlineSiteDto,
        player: &str,
        speeds: &[SpeedDto],
        rated_only: bool,
        reached: i64,
    ) -> Self {
        let mut speeds = speeds.to_vec();
        speeds.sort_unstable();
        speeds.dedup();
        DownloadMark { site, player: player.to_ascii_lowercase(), speeds, rated_only, reached }
    }

    fn same_games(&self, other: &DownloadMark) -> bool {
        (self.site, &self.player, &self.speeds, self.rated_only)
            == (other.site, &other.player, &other.speeds, other.rated_only)
    }

    /// Whether the games it counts take in all of `wanted`'s.
    fn covers(&self, wanted: &DownloadMark) -> bool {
        self.site == wanted.site
            && self.player == wanted.player
            && (self.speeds.is_empty()
                || (!wanted.speeds.is_empty() && wanted.speeds.iter().all(|s| self.speeds.contains(s))))
            && (!self.rated_only || wanted.rated_only)
    }
}

impl DbView {
    /// Where "new ones only" starts for the games `wanted` names (its
    /// `reached` unused): how far the downloads that took them all in got.
    pub fn download_mark(&self, wanted: &DownloadMark) -> Option<i64> {
        self.downloads.iter().filter(|m| m.covers(wanted)).map(|m| m.reached).max()
    }

    /// Notes how far a download got; a mark only moves on.
    pub fn note_download(&mut self, mark: DownloadMark) {
        let reached = match self.downloads.iter().position(|m| m.same_games(&mark)) {
            Some(i) => self.downloads.remove(i).reached.max(mark.reached),
            None => mark.reached,
        };
        self.downloads.insert(0, DownloadMark { reached, ..mark });
        self.downloads.truncate(MAX_MARKS);
    }
}

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
    /// The welcome guide (QOL-08) was finished or dismissed.
    pub welcome_done: bool,
    /// The game list's look per database (QOL-07), most recently used first.
    pub database_views: Vec<DbView>,
    /// Preferences owned by the frontend (board orientation, pane sizes…).
    pub ui: serde_json::Map<String, serde_json::Value>,
    /// Counts the changes to `ui` since start-up (not saved). Every window
    /// keeps its own copy of the preferences (DB-07), and two changes to the
    /// same one can reach a window in either order, so each carries the
    /// revision it was saved at and the window keeps the newer.
    #[serde(skip)]
    pub ui_revision: u64,
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Settings>(&bytes)
                .unwrap_or_else(|e| {
                    log::warn!("ignoring unreadable settings {}: {e}", path.display());
                    Settings::default()
                })
                .simplified(),
            Err(_) => Settings::default(),
        }
    }

    /// Drops the `\\?\` prefix Windows' `canonicalize` put on paths before
    /// they went through `dunce`, so old settings show `C:\…` and Recent
    /// doesn't list a database twice. A no-op elsewhere.
    fn simplified(mut self) -> Settings {
        let mut recent: Vec<PathBuf> = Vec::with_capacity(self.recent_databases.len());
        for p in self.recent_databases.drain(..) {
            let p = dunce::simplified(&p).to_path_buf();
            if !recent.contains(&p) {
                recent.push(p);
            }
        }
        self.recent_databases = recent;
        self.reference_database = self.reference_database.map(|p| dunce::simplified(&p).to_path_buf());
        for e in &mut self.engines {
            e.path = dunce::simplified(&e.path).to_path_buf();
        }
        let mut views: Vec<DbView> = Vec::with_capacity(self.database_views.len());
        for mut v in self.database_views.drain(..) {
            v.path = dunce::simplified(&v.path).to_path_buf();
            if v.filter.as_ref().is_some_and(|f| f.steps.contains(&FilterStep::Unknown)) {
                v.filter = None;
            }
            if !views.iter().any(|seen| seen.path == v.path) {
                views.push(v);
            }
        }
        self.database_views = views;
        self
    }

    /// Writes the settings atomically (temp file + rename).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, dir.join(FILE))
    }

    /// Sets one frontend preference (`null` removes it) and returns the
    /// revision it's saved at (see `ui_revision`).
    pub fn set_ui(&mut self, key: String, value: serde_json::Value) -> u64 {
        if value.is_null() {
            self.ui.remove(&key);
        } else {
            self.ui.insert(key, value);
        }
        self.ui_revision += 1;
        self.ui_revision
    }

    /// Whether to open the welcome guide at start-up: on a first run, but
    /// not for someone who used the app before the guide existed (they have
    /// recent databases).
    pub fn needs_welcome(&self) -> bool {
        !self.welcome_done && self.recent_databases.is_empty()
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

    /// Adds the engine at `path` unless one with that path is configured.
    /// Callers probe the engine before calling this, so it checks again:
    /// two calls at once (the first `list_engines` on a fresh install, say)
    /// would otherwise both add it.
    pub fn add_engine(
        &mut self,
        path: PathBuf,
        name: String,
        declared: Vec<chess_engine::UciOption>,
        detected: bool,
    ) -> bool {
        if self.engines.iter().any(|e| e.path == path) {
            return false;
        }
        let id = new_id(&self.engines);
        self.engines.push(EngineConfig { id, name, path, detected, declared, options: Vec::new() });
        true
    }

    pub fn note_recent(&mut self, path: &Path) {
        self.recent_databases.retain(|p| p != path);
        self.recent_databases.insert(0, path.to_path_buf());
        self.recent_databases.truncate(MAX_RECENT);
    }

    /// The saved list view of the database at `path`, if there is one.
    pub fn db_view(&self, path: &Path) -> Option<&DbView> {
        self.database_views.iter().find(|v| v.path == path)
    }

    /// The list view of the database at `path` to change, made if needed
    /// and moved to the front. Views beyond the most recent `MAX_VIEWS` go.
    pub fn db_view_mut(&mut self, path: &Path) -> &mut DbView {
        let view = match self.database_views.iter().position(|v| v.path == path) {
            Some(i) => self.database_views.remove(i),
            None => DbView { path: path.to_path_buf(), ..DbView::default() },
        };
        self.database_views.insert(0, view);
        self.database_views.truncate(MAX_VIEWS);
        &mut self.database_views[0]
    }

    /// Forgets the list view of the database at `path` (a new database there).
    pub fn forget_db_view(&mut self, path: &Path) {
        self.database_views.retain(|v| v.path != path);
    }

    /// Lists a database created for copying games into (QOL-04) among the
    /// recent ones, second: the first is the one reopened at start-up.
    /// One that's listed already stays where it is.
    pub fn remember_database(&mut self, path: &Path) {
        if !self.recent_databases.iter().any(|p| p == path) {
            let at = self.recent_databases.len().min(1);
            self.recent_databases.insert(at, path.to_path_buf());
            self.recent_databases.truncate(MAX_RECENT);
        }
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
        s.welcome_done = true;
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

    #[test]
    fn numbers_each_change_to_the_frontend_prefs() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        assert_eq!(s.set_ui("appTheme".into(), "dark".into()), 1);
        assert_eq!(s.set_ui("libraryFavorites".into(), serde_json::json!(["/a.chessdb"])), 2);
        assert_eq!(s.set_ui("appTheme".into(), serde_json::Value::Null), 3, "a removal counts too");
        assert!(!s.ui.contains_key("appTheme"));
        assert_eq!(s.ui["libraryFavorites"], serde_json::json!(["/a.chessdb"]));

        // The revision lasts for the session only: it isn't saved.
        s.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(!text.contains("uiRevision"), "{text}");
        let loaded = Settings::load(dir.path());
        assert_eq!(loaded.ui_revision, 0);
        assert_eq!(loaded.ui, s.ui);
    }

    #[test]
    fn welcomes_new_users_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::load(dir.path());
        assert!(s.needs_welcome(), "first run");
        s.welcome_done = true;
        s.save(dir.path()).unwrap();
        assert!(!Settings::load(dir.path()).needs_welcome(), "finished");

        // Settings from before the guide existed: someone who has opened
        // databases isn't new.
        std::fs::write(dir.path().join(FILE), br#"{"recentDatabases": ["/a.chessdb"]}"#).unwrap();
        assert!(!Settings::load(dir.path()).needs_welcome());
        std::fs::write(dir.path().join(FILE), br#"{"engines": []}"#).unwrap();
        assert!(Settings::load(dir.path()).needs_welcome());
    }

    #[test]
    fn keeps_the_list_views_of_recent_databases() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        let view = s.db_view_mut(Path::new("/a.chessdb"));
        view.sort = Some(SortSpecDto { key: crate::dto::SortKeyDto::Date, descending: true });
        view.columns.insert("white".into(), 180);
        view.filter = Some(SavedFilter {
            steps: vec![
                FilterStep::Name { table: NameKindDto::Player, name: "Kasparov, Garry".into() },
                FilterStep::Header {
                    query: HeaderQueryDto { year_from: Some(1999), ..Default::default() },
                    quick: false,
                },
                FilterStep::Position { fen: "8/8/8/8/8/8/8/K6k w - - 0 1".into(), full: true },
            ],
            description: "Player: Kasparov, Garry, then from 1999".into(),
        });
        s.db_view_mut(Path::new("/b.chessdb"));
        s.save(dir.path()).unwrap();
        let loaded = Settings::load(dir.path());
        assert_eq!(loaded, s);
        assert_eq!(loaded.db_view(Path::new("/a.chessdb")).unwrap().columns["white"], 180);

        // Most recently used first, and only so many.
        s.db_view_mut(Path::new("/a.chessdb"));
        assert_eq!(s.database_views[0].path, PathBuf::from("/a.chessdb"));
        assert_eq!(s.database_views[0].columns["white"], 180, "moving keeps it");
        for i in 0..MAX_VIEWS {
            s.db_view_mut(&PathBuf::from(format!("/{i}.chessdb")));
        }
        assert_eq!(s.database_views.len(), MAX_VIEWS);
        assert!(s.db_view(Path::new("/a.chessdb")).is_none(), "the oldest went");
        s.forget_db_view(Path::new("/0.chessdb"));
        assert!(s.db_view(Path::new("/0.chessdb")).is_none());

        // Old settings files have no views.
        std::fs::write(dir.path().join(FILE), br#"{"recentDatabases": ["/c"]}"#).unwrap();
        assert!(Settings::load(dir.path()).database_views.is_empty());
    }

    #[test]
    fn a_copy_target_is_remembered_behind_the_open_database() {
        let mut s = Settings::default();
        s.remember_database(Path::new("/clip.chessdb"));
        assert_eq!(s.recent_databases, [PathBuf::from("/clip.chessdb")]);
        s.note_recent(Path::new("/a.chessdb"));
        s.remember_database(Path::new("/new.chessdb"));
        s.remember_database(Path::new("/clip.chessdb"));
        let recent: Vec<&str> = s.recent_databases.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(recent, ["/a.chessdb", "/new.chessdb", "/clip.chessdb"]);
    }

    #[test]
    fn adds_an_engine_path_once() {
        let mut s = Settings::default();
        assert!(s.add_engine("/x/stockfish".into(), "Stockfish".into(), Vec::new(), true));
        assert!(!s.add_engine("/x/stockfish".into(), "Stockfish".into(), Vec::new(), true));
        assert!(s.add_engine("/y/stockfish".into(), "Stockfish".into(), Vec::new(), false));
        let ids: Vec<&str> = s.engines.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["engine-1", "engine-2"]);
    }

    #[cfg(windows)]
    #[test]
    fn drops_extended_length_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let json = br#"{
            "recentDatabases": ["\\\\?\\C:\\x\\a.chessdb", "C:\\x\\a.chessdb", "C:\\x\\b.chessdb"],
            "referenceDatabase": "\\\\?\\C:\\x\\b.chessdb",
            "engines": [{"id": "engine-1", "name": "Stockfish", "path": "\\\\?\\C:\\sf\\stockfish.exe"}]
        }"#;
        std::fs::write(dir.path().join(FILE), json).unwrap();
        let s = Settings::load(dir.path());
        assert_eq!(s.recent_databases, [PathBuf::from(r"C:\x\a.chessdb"), PathBuf::from(r"C:\x\b.chessdb")]);
        assert_eq!(s.reference_database, Some(PathBuf::from(r"C:\x\b.chessdb")));
        assert_eq!(s.engines[0].path, PathBuf::from(r"C:\sf\stockfish.exe"));
    }

    /// A filter step this version doesn't know (DB-08's first draft saved a
    /// `duplicates` one) loses only that filter, not the whole file, which
    /// the next save would overwrite.
    #[test]
    fn a_filter_step_from_a_newer_version_drops_only_that_filter() {
        let dir = tempfile::tempdir().unwrap();
        let json = br#"{
            "engines": [{"id": "engine-1", "name": "Stockfish", "path": "/sf/stockfish"}],
            "recentDatabases": ["/x/a.chessdb", "/x/b.chessdb"],
            "referenceDatabase": "/x/b.chessdb",
            "welcomeDone": true,
            "databaseViews": [
                {"path": "/x/a.chessdb", "columns": {"white": 120},
                 "filter": {"steps": [{"kind": "duplicates", "fuzzy": true}], "description": "Duplicates"}},
                {"path": "/x/b.chessdb",
                 "filter": {"steps": [{"kind": "name", "table": "player", "name": "Carlsen, Magnus"},
                                      {"kind": "someday", "x": [1, 2]}], "description": "Later"}},
                {"path": "/x/c.chessdb",
                 "filter": {"steps": [{"kind": "position", "fen": "8/8/8/8/8/8/8/K1k5 w - - 0 1"}],
                            "description": "A position"}}
            ],
            "ui": {"duplicatesFuzzy": true, "theme": "dark"}
        }"#;
        std::fs::write(dir.path().join(FILE), json).unwrap();
        let s = Settings::load(dir.path());
        assert_eq!(s.engines.len(), 1);
        assert_eq!(s.recent_databases.len(), 2);
        assert_eq!(s.reference_database, Some(PathBuf::from("/x/b.chessdb")));
        assert!(s.welcome_done);
        assert_eq!(s.ui.len(), 2);
        let views = &s.database_views;
        assert_eq!(views.len(), 3);
        assert_eq!((views[0].filter.as_ref(), views[0].columns.get("white")), (None, Some(&120)));
        assert_eq!(views[1].filter, None);
        assert_eq!(views[2].filter.as_ref().map(|f| f.description.as_str()), Some("A position"));
    }

    /// NET-01: "new ones only" starts where a download of the same games,
    /// or of more, got to; never from another player's games or other
    /// time controls (review round 1).
    #[test]
    fn new_ones_start_where_the_same_games_got_to() {
        use OnlineSiteDto::{ChessCom, Lichess};
        use SpeedDto::{Blitz, Bullet};
        let wanted = |site, player: &str, speeds: &[SpeedDto], rated| {
            DownloadMark::new(site, player, speeds, rated, 0)
        };
        let mut view = DbView::default();
        view.note_download(DownloadMark::new(Lichess, "Alice", &[Blitz], false, 100));
        assert_eq!(view.download_mark(&wanted(Lichess, "alice", &[Blitz], false)), Some(100), "any case");
        assert_eq!(
            view.download_mark(&wanted(Lichess, "alice", &[Blitz], true)),
            Some(100),
            "rated: fewer games"
        );
        assert_eq!(view.download_mark(&wanted(Lichess, "bob", &[Blitz], false)), None, "her opponent");
        assert_eq!(view.download_mark(&wanted(Lichess, "alice", &[Bullet], false)), None, "other games");
        assert_eq!(view.download_mark(&wanted(Lichess, "alice", &[], false)), None, "more games");
        assert_eq!(view.download_mark(&wanted(ChessCom, "alice", &[Blitz], false)), None, "another site");

        view.note_download(DownloadMark::new(Lichess, "alice", &[], true, 200));
        assert_eq!(view.download_mark(&wanted(Lichess, "alice", &[Bullet], true)), Some(200));
        assert_eq!(view.download_mark(&wanted(Lichess, "alice", &[Bullet], false)), None, "casual games too");
        assert_eq!(view.download_mark(&wanted(Lichess, "alice", &[Blitz], true)), Some(200), "the furthest");

        // A mark only moves on, and the same games keep one mark.
        view.note_download(DownloadMark::new(Lichess, "ALICE", &[Blitz, Blitz], false, 50));
        assert_eq!(view.download_mark(&wanted(Lichess, "alice", &[Blitz], false)), Some(100));
        assert_eq!(view.downloads.len(), 2);
        assert_eq!(view.downloads[0].reached, 100);

        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(serde_json::from_str::<DbView>(&json).unwrap(), view);
        assert_eq!(serde_json::from_str::<DbView>("{}").unwrap().downloads, [], "views saved before");
    }
}
