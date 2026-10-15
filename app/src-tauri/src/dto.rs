//! Serializable types shared with the frontend. TypeScript definitions are
//! generated into `app/src/lib/bindings/` by `npm run bindings`.

use std::collections::{BTreeMap, HashMap};

use chess_core::game::{GameResult, Headers, Shape, ShapeColor};
use chess_core::shakmaty::Square;
use chess_db::{DbInfo, EloMode, GameRow, HeaderQuery, ResultFilter, SortKey, SortSpec};
use chess_jobs::{JobState, JobStatus};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::settings::{DbView, FilterStep};

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DbInfoDto {
    pub handle: u32,
    pub name: String,
    pub path: String,
    pub games: u32,
    pub players: u32,
    pub events: u32,
    #[ts(type = "number")]
    pub size_bytes: u64,
    /// The search results the list is limited to, if any.
    pub filter: Option<FilterDto>,
    pub index: IndexStatusDto,
    /// Games marked as deleted (removed for good by compacting).
    pub deleted: u32,
}

/// The reference database (DB-13) from the settings.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReferenceDbDto {
    pub path: String,
    pub name: String,
    /// `None` when it couldn't be opened (see `error`).
    pub info: Option<DbInfoDto>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FilterDto {
    pub count: u32,
    pub description: String,
}

/// How a database's game list looked last time (QOL-07), to put back when
/// it's opened.
#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DbViewDto {
    /// `None` for the natural order.
    pub sort: Option<SortSpecDto>,
    /// Widths in CSS pixels of the columns the user resized, by column id.
    pub columns: BTreeMap<String, u32>,
    /// Description of the saved filter, which `restore_filter` makes again.
    pub filter: Option<String>,
    /// Text for the quick filter box, when the saved filter is the quick filter.
    pub quick_text: String,
    /// The saved filter's last header search, to start the search dialog from.
    pub last_query: Option<HeaderQueryDto>,
}

impl From<&DbView> for DbViewDto {
    fn from(v: &DbView) -> DbViewDto {
        let steps = v.filter.as_ref().map(|f| f.steps.as_slice()).unwrap_or_default();
        let quick_text = match steps {
            [FilterStep::Header { query, quick: true }] => query.text.clone(),
            _ => String::new(),
        };
        let last_query = steps.iter().rev().find_map(|s| match s {
            FilterStep::Header { query, quick: false } => Some(query.clone()),
            _ => None,
        });
        DbViewDto {
            sort: v.sort,
            columns: v.columns.clone(),
            filter: v.filter.as_ref().map(|f| f.description.clone()),
            quick_text,
            last_query,
        }
    }
}

impl DbInfoDto {
    pub fn new(
        handle: u32,
        info: DbInfo,
        filter: Option<FilterDto>,
        index: IndexStatusDto,
        deleted: u32,
    ) -> DbInfoDto {
        DbInfoDto {
            deleted,
            filter,
            index,
            handle,
            name: info.name,
            path: info.path.to_string_lossy().into_owned(),
            games: info.games,
            players: info.players,
            events: info.events,
            size_bytes: info.size_bytes,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameRowDto {
    pub id: u32,
    pub white: String,
    pub white_elo: Option<u16>,
    pub black: String,
    pub black_elo: Option<u16>,
    pub result: String,
    pub date: String,
    pub event: String,
    pub site: String,
    pub round: String,
    pub eco: String,
    pub annotator: String,
    pub plies: u16,
    pub deleted: bool,
    pub has_variations: bool,
    pub has_comments: bool,
    pub has_annotations: bool,
    pub has_setup: bool,
    /// Classified opening (empty until the position index is built).
    pub opening: String,
    pub opening_eco: String,
}

impl From<GameRow> for GameRowDto {
    fn from(r: GameRow) -> GameRowDto {
        GameRowDto {
            id: r.id,
            white: r.white,
            white_elo: r.white_elo,
            black: r.black,
            black_elo: r.black_elo,
            result: r.result.as_str().to_owned(),
            date: r.date,
            event: r.event,
            site: r.site,
            round: r.round,
            eco: r.eco,
            annotator: r.annotator,
            plies: r.plies,
            deleted: r.deleted,
            has_variations: r.has_variations,
            has_comments: r.has_comments,
            has_annotations: r.has_annotations,
            has_setup: r.has_setup,
            opening: r.opening.map(|o| o.name.to_owned()).unwrap_or_default(),
            opening_eco: r.opening.map(|o| o.eco.to_owned()).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GamePage {
    pub total: u32,
    pub offset: u32,
    pub rows: Vec<GameRowDto>,
}

/// Selected rows of the game list as shown: `[start, end)` row ranges in
/// `sort` order, of the filtered list when a filter is active. A selection
/// of a million rows is still a few numbers over IPC (DB-06, QOL-04).
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListSelectionDto {
    pub sort: SortSpecDto,
    pub ranges: Vec<(u32, u32)>,
}

/// Copy games to another database, or move them (the originals are marked
/// as deleted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CopyModeDto {
    Copy,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SortKeyDto {
    Id,
    White,
    Black,
    WhiteElo,
    BlackElo,
    Date,
    Event,
    Site,
    Round,
    Result,
    Eco,
    Plies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SortSpecDto {
    pub key: SortKeyDto,
    pub descending: bool,
}

impl From<SortSpecDto> for SortSpec {
    fn from(s: SortSpecDto) -> SortSpec {
        let key = match s.key {
            SortKeyDto::Id => SortKey::Id,
            SortKeyDto::White => SortKey::White,
            SortKeyDto::Black => SortKey::Black,
            SortKeyDto::WhiteElo => SortKey::WhiteElo,
            SortKeyDto::BlackElo => SortKey::BlackElo,
            SortKeyDto::Date => SortKey::Date,
            SortKeyDto::Event => SortKey::Event,
            SortKeyDto::Site => SortKey::Site,
            SortKeyDto::Round => SortKey::Round,
            SortKeyDto::Result => SortKey::Result,
            SortKeyDto::Eco => SortKey::Eco,
            SortKeyDto::Plies => SortKey::Plies,
        };
        SortSpec::new(key, s.descending)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum JobStateDto {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobDto {
    #[ts(type = "number")]
    pub id: u64,
    pub kind: String,
    pub label: String,
    pub state: JobStateDto,
    #[ts(type = "number")]
    pub done: u64,
    #[ts(type = "number")]
    pub total: u64,
    pub message: String,
    pub error: Option<String>,
}

impl From<&JobStatus> for JobDto {
    fn from(s: &JobStatus) -> JobDto {
        let (state, error) = match &s.state {
            JobState::Running => (JobStateDto::Running, None),
            JobState::Done => (JobStateDto::Done, None),
            JobState::Failed(e) => (JobStateDto::Failed, Some(e.clone())),
            JobState::Cancelled => (JobStateDto::Cancelled, None),
        };
        JobDto {
            id: s.id,
            kind: s.kind.clone(),
            label: s.label.clone(),
            state,
            done: s.done,
            total: s.total,
            message: s.message.clone(),
            error,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HeadersDto {
    pub event: String,
    pub site: String,
    pub date: String,
    pub round: String,
    pub white: String,
    pub black: String,
    pub result: String,
    pub white_elo: Option<u16>,
    pub black_elo: Option<u16>,
    pub eco: String,
    pub annotator: String,
    pub extra: Vec<(String, String)>,
}

impl From<&Headers> for HeadersDto {
    fn from(h: &Headers) -> HeadersDto {
        HeadersDto {
            event: h.event.clone(),
            site: h.site.clone(),
            date: h.date.clone(),
            round: h.round.clone(),
            white: h.white.clone(),
            black: h.black.clone(),
            result: h.result.as_str().to_owned(),
            white_elo: h.white_elo,
            black_elo: h.black_elo,
            eco: h.eco.clone(),
            annotator: h.annotator.clone(),
            extra: h.extra.clone(),
        }
    }
}

impl From<HeadersDto> for Headers {
    fn from(d: HeadersDto) -> Headers {
        let clean = |s: String| s.trim().to_owned();
        let extra = d
            .extra
            .into_iter()
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .filter(|(k, _)| !k.is_empty() && !matches!(k.as_str(), "FEN" | "SetUp"))
            .collect();
        Headers {
            event: clean(d.event),
            site: clean(d.site),
            date: clean(d.date),
            round: clean(d.round),
            white: clean(d.white),
            black: clean(d.black),
            result: GameResult::parse(&d.result),
            white_elo: d.white_elo.filter(|&e| e > 0),
            black_elo: d.black_elo.filter(|&e| e > 0),
            eco: clean(d.eco).to_uppercase(),
            annotator: clean(d.annotator),
            extra,
        }
    }
}

/// A chessground-compatible drawing: arrow (`dest` set) or circled square.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShapeDto {
    pub orig: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dest: Option<String>,
    pub brush: String,
}

impl From<&Shape> for ShapeDto {
    fn from(s: &Shape) -> ShapeDto {
        let brush = match s.color {
            ShapeColor::Green => "green",
            ShapeColor::Red => "red",
            ShapeColor::Yellow => "yellow",
            ShapeColor::Blue => "blue",
        };
        ShapeDto { orig: s.from.to_string(), dest: s.to.map(|t| t.to_string()), brush: brush.into() }
    }
}

impl TryFrom<ShapeDto> for Shape {
    type Error = AppError;

    fn try_from(d: ShapeDto) -> Result<Shape, AppError> {
        let square =
            |s: &str| s.parse::<Square>().map_err(|_| AppError::invalid(format!("bad square {s:?}")));
        let color = match d.brush.as_str() {
            "green" => ShapeColor::Green,
            "red" => ShapeColor::Red,
            "yellow" => ShapeColor::Yellow,
            "blue" => ShapeColor::Blue,
            other => return Err(AppError::invalid(format!("unsupported brush {other:?}"))),
        };
        let from = square(&d.orig)?;
        let to = d.dest.as_deref().map(square).transpose()?.filter(|&t| t != from);
        Ok(Shape { color, from, to })
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotationMoveDto {
    pub id: u32,
    pub san: String,
    /// The move in UCI (castling as `e1g1`), for arrows on the board.
    pub uci: String,
    pub move_number: u32,
    pub white: bool,
    pub nags: Vec<u8>,
    pub comment_before: String,
    pub comment_after: String,
    pub clock: Option<String>,
    pub eval: Option<String>,
    /// Alternatives to this move, each a line starting with the alternative.
    pub variations: Vec<Vec<NotationMoveDto>>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotationDto {
    pub root_comment: String,
    pub moves: Vec<NotationMoveDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GameStatusDto {
    Ongoing,
    Checkmate,
    Stalemate,
    InsufficientMaterial,
    FiftyMoves,
    Threefold,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export)]
pub struct SourceDto {
    pub db: u32,
    pub game: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameViewDto {
    pub session: u32,
    pub source: Option<SourceDto>,
    pub fen: String,
    /// `"white"` or `"black"`.
    pub turn: String,
    pub cursor: u32,
    /// Node ids from the first move to the cursor.
    pub cursor_path: Vec<u32>,
    pub in_variation: bool,
    pub last_move: Option<Vec<String>>,
    pub check: bool,
    pub dests: HashMap<String, Vec<String>>,
    pub status: GameStatusDto,
    pub winner: Option<String>,
    pub shapes: Vec<ShapeDto>,
    pub headers: HeadersDto,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    #[ts(type = "number")]
    pub revision: u64,
    /// Only sent when the game content changed since the last view.
    pub notation: Option<NotationDto>,
    /// The game's opening (deepest named position of its main line).
    pub opening: Option<OpeningDto>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum PlayResultDto {
    /// `dropped`: moves an insert removed because they were no longer legal.
    Moved {
        view: Box<GameViewDto>,
        dropped: u32,
    },
    NeedsChoice,
    NeedsPromotion,
    Illegal,
}

/// A legal move, as typed moves are matched against it (UI-02).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LegalMoveDto {
    /// With check marks: `Nf3`, `exd8=Q+`, `O-O`.
    pub san: String,
    /// Castling as the king's move: `e1g1`.
    pub uci: String,
}

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VariationPolicyDto {
    NewVariation,
    NewMainLine,
    Overwrite,
    Insert,
}

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StepDto {
    Forward,
    Back,
    Start,
    End,
}

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CommentSlotDto {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SaveModeDto {
    Replace,
    New,
}

/// How a board image (QOL-06) looks: the frontend passes the board's
/// appearance. The position, last move, check and drawings come from the
/// session.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BoardImageDto {
    /// Width and height in pixels.
    pub size: u32,
    /// The side at the bottom: `"white"` or `"black"`.
    pub orientation: String,
    /// Square colours, `#rrggbb`.
    pub light: String,
    pub dark: String,
    pub coordinates: bool,
    /// Tint the squares of the last move.
    pub last_move: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImageFormatDto {
    Png,
    Svg,
}

/// An engine evaluation from White's point of view: centipawns or mate in
/// N moves (negative: Black mates).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct EvalDto {
    pub cp: Option<i32>,
    pub mate: Option<i32>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PvMoveDto {
    pub uci: String,
    pub san: String,
    pub move_number: u32,
    pub white: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PvLineDto {
    pub multipv: u32,
    pub eval: EvalDto,
    /// `"exact"`, `"lower"` (at least this good for White) or `"upper"`.
    pub bound: String,
    /// Win/draw/loss per mille for White, when the engine reports it.
    pub wdl: Option<Vec<u32>>,
    pub depth: u32,
    pub moves: Vec<PvMoveDto>,
}

/// One analysis update (event `analysis`). `error` is set when the engine
/// failed; the analysis has stopped then.
#[derive(Debug, Clone, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisDto {
    pub session: u32,
    /// The analysed position.
    pub fen: String,
    pub depth: u32,
    pub seldepth: u32,
    #[ts(type = "number")]
    pub nodes: u64,
    #[ts(type = "number")]
    pub nps: u64,
    #[ts(type = "number")]
    pub time_ms: u64,
    pub hashfull: u32,
    pub finished: bool,
    pub lines: Vec<PvLineDto>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisStatusDto {
    pub running: bool,
    pub engine_id: Option<String>,
    pub engine_name: Option<String>,
    pub session: Option<u32>,
    pub multipv: u32,
}

/// A UCI option as the engine declares it.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum UciOptionDto {
    Check {
        name: String,
        default: bool,
    },
    Spin {
        name: String,
        #[ts(type = "number")]
        default: i64,
        #[ts(type = "number")]
        min: i64,
        #[ts(type = "number")]
        max: i64,
    },
    Combo {
        name: String,
        default: String,
        vars: Vec<String>,
    },
    Button {
        name: String,
    },
    String {
        name: String,
        default: String,
    },
}

impl From<&chess_engine::UciOption> for UciOptionDto {
    fn from(o: &chess_engine::UciOption) -> UciOptionDto {
        use chess_engine::UciOption as O;
        match o.clone() {
            O::Check { name, default } => UciOptionDto::Check { name, default },
            O::Spin { name, default, min, max } => UciOptionDto::Spin { name, default, min, max },
            O::Combo { name, default, vars } => UciOptionDto::Combo { name, default, vars },
            O::Button { name } => UciOptionDto::Button { name },
            O::String { name, default } => UciOptionDto::String { name, default },
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EngineDto {
    pub id: String,
    pub name: String,
    pub path: String,
    /// Found automatically (bundled or on PATH).
    pub detected: bool,
    pub is_default: bool,
    pub declared: Vec<UciOptionDto>,
    /// Values changed from the engine's defaults.
    pub options: Vec<(String, String)>,
}

impl EngineDto {
    pub fn new(e: &crate::settings::EngineConfig, default_id: Option<&str>) -> EngineDto {
        EngineDto {
            id: e.id.clone(),
            name: e.name.clone(),
            path: e.path.to_string_lossy().into_owned(),
            detected: e.detected,
            is_default: default_id == Some(e.id.as_str()),
            declared: e.declared.iter().map(UciOptionDto::from).collect(),
            options: e.options.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentDbDto {
    pub path: String,
    pub name: String,
    pub exists: bool,
    /// `None` when `exists` is false, or it couldn't be read (not a database any more, say).
    pub games: Option<u32>,
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
}

/// The frontend's saved preferences (INF-02), as of `revision` (see
/// `Settings::ui_revision`).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UiPrefsDto {
    #[ts(type = "Record<string, unknown>")]
    pub values: serde_json::Map<String, serde_json::Value>,
    #[ts(type = "number")]
    pub revision: u64,
}

/// A preference one window saved, sent to every window (the `ui-pref`
/// event, DB-07) so their copies stay current.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UiPrefChangeDto {
    pub key: String,
    /// Null when the preference was removed.
    #[ts(type = "unknown")]
    pub value: serde_json::Value,
    #[ts(type = "number")]
    pub revision: u64,
}

/// State of the welcome guide (QOL-08).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OnboardingDto {
    /// Open the guide at start-up: a first run.
    pub show_welcome: bool,
    /// Where the sample database is, or will be created.
    pub sample_path: String,
    /// Games in the sample database as shipped.
    pub sample_games: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ResultFilterDto {
    WhiteWins,
    BlackWins,
    Draw,
    Decisive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EloModeDto {
    #[default]
    Both,
    Either,
    Average,
}

/// Header search criteria (SRCH-01); empty fields match everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct HeaderQueryDto {
    pub white: String,
    pub black: String,
    pub ignore_colors: bool,
    /// Any player, event, site or annotator (the quick filter).
    pub text: String,
    pub event: String,
    pub site: String,
    pub annotator: String,
    pub year_from: Option<u16>,
    pub year_to: Option<u16>,
    pub eco_from: String,
    pub eco_to: String,
    pub result: Option<ResultFilterDto>,
    pub elo_min: Option<u16>,
    pub elo_max: Option<u16>,
    pub elo_mode: EloModeDto,
    pub moves_min: Option<u16>,
    pub moves_max: Option<u16>,
    pub annotated_only: bool,
    pub skip_deleted: bool,
}

impl From<HeaderQueryDto> for HeaderQuery {
    fn from(d: HeaderQueryDto) -> HeaderQuery {
        HeaderQuery {
            white: d.white,
            black: d.black,
            ignore_colors: d.ignore_colors,
            text: d.text,
            event: d.event,
            site: d.site,
            annotator: d.annotator,
            year_from: d.year_from,
            year_to: d.year_to,
            eco_from: d.eco_from,
            eco_to: d.eco_to,
            result: d.result.map(|r| match r {
                ResultFilterDto::WhiteWins => ResultFilter::WhiteWins,
                ResultFilterDto::BlackWins => ResultFilter::BlackWins,
                ResultFilterDto::Draw => ResultFilter::Draw,
                ResultFilterDto::Decisive => ResultFilter::Decisive,
            }),
            elo_min: d.elo_min,
            elo_max: d.elo_max,
            elo_mode: match d.elo_mode {
                EloModeDto::Both => EloMode::Both,
                EloModeDto::Either => EloMode::Either,
                EloModeDto::Average => EloMode::Average,
            },
            moves_min: d.moves_min,
            moves_max: d.moves_max,
            annotated_only: d.annotated_only,
            skip_deleted: d.skip_deleted,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpeningDto {
    pub eco: String,
    pub name: String,
}

impl From<&chess_core::openings::Opening> for OpeningDto {
    fn from(o: &chess_core::openings::Opening) -> OpeningDto {
        OpeningDto { eco: o.eco.to_owned(), name: o.name.to_owned() }
    }
}

/// State of a database's position index (SRCH-04).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IndexStatusDto {
    pub built: bool,
    /// Games `0..covered` are indexed.
    pub covered: u32,
    pub up_to_date: bool,
    pub max_ply: u16,
    #[ts(type = "number")]
    pub size_bytes: u64,
}

/// One row of the opening tree: a move (or, with `san` unset, games that
/// ended in the position, or the total).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TreeRowDto {
    pub uci: Option<String>,
    pub san: Option<String>,
    pub games: u32,
    pub white_wins: u32,
    pub draws: u32,
    pub black_wins: u32,
    /// Score for the side to move, 0…1.
    pub score: f64,
    pub avg_elo: Option<u32>,
    /// Performance rating of the side to move with this move.
    pub performance: Option<i32>,
    pub last_year: Option<u16>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TreeDto {
    pub fen: String,
    pub opening: Option<OpeningDto>,
    pub total: TreeRowDto,
    pub moves: Vec<TreeRowDto>,
    /// The index has no data yet (build it first).
    pub indexed: bool,
    /// Some new or edited games couldn't be included (index update pending).
    pub partial: bool,
    /// Games may reach the position later than the indexed plies, so some
    /// can be missing (a full search finds them): its move number is past
    /// them, or the game starts from a set-up position.
    pub beyond_index: bool,
    pub max_ply: u16,
}

/// Whether a move is played more (↑) or less (↓) lately than over all
/// years (the Reference tab's Hot, OPEN-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TrendDto {
    Up,
    Down,
}

impl From<chess_db::Trend> for TrendDto {
    fn from(t: chess_db::Trend) -> TrendDto {
        match t {
            chess_db::Trend::Up => TrendDto::Up,
            chess_db::Trend::Down => TrendDto::Down,
        }
    }
}

/// How much a move is played lately (OPEN-14 Hot).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HotDto {
    /// Games with the move from the recent years.
    pub recent: u32,
    /// Its share of the position's recent games (the bar), 0…1.
    pub share: f64,
    /// Its share of the position's games over all years, 0…1.
    pub all_share: f64,
    pub trend: Option<TrendDto>,
}

/// One of the strongest players who chose a move, at their Elo then.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BestPlayerDto {
    pub name: String,
    /// What the column shows: the surname of a "Surname, First name".
    pub surname: String,
    pub elo: u16,
}

/// A row of the Reference tab's moves table: the opening tree's columns
/// plus Hot and Best players.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReferenceMoveDto {
    pub row: TreeRowDto,
    pub hot: Option<HotDto>,
    pub best: Vec<BestPlayerDto>,
}

/// A game reaching the position (the Reference tab's top games).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TopGameDto {
    pub id: u32,
    pub white: String,
    pub white_elo: Option<u16>,
    pub black: String,
    pub black_elo: Option<u16>,
    pub result: String,
    pub year: Option<u16>,
    pub event: String,
    /// Ply where the game first reaches the position.
    pub ply: u16,
}

/// The Reference tab (OPEN-14) at a position: moves with Hot and Best
/// players, and the top games. The lines come separately
/// (`ReferenceLinesDto`), as they take longer.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReferenceDto {
    pub fen: String,
    pub opening: Option<OpeningDto>,
    pub total: TreeRowDto,
    pub moves: Vec<ReferenceMoveDto>,
    /// The years Hot counts as recent (none when no game has a year).
    pub recent_from: Option<u16>,
    pub recent_to: Option<u16>,
    /// Strongest first (average Elo of both players), at most 100.
    pub top_games: Vec<TopGameDto>,
    /// As in `TreeDto`.
    pub indexed: bool,
    pub partial: bool,
    pub beyond_index: bool,
    pub max_ply: u16,
}

/// A position of a line or a game, as the mini board shows it.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LinePositionDto {
    pub fen: String,
    /// The move that led here, in SAN and UCI (castling as `e1g1`); none for
    /// the first position.
    pub san: Option<String>,
    pub uci: Option<String>,
    /// Plies from the first position.
    pub ply: u32,
    /// Number of the move that led here, and whether White played it.
    pub move_number: u32,
    pub white: bool,
}

/// A line from the Reference tab's main variations (OPEN-14).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReferenceLineDto {
    /// The position on the board first, then one per move.
    pub positions: Vec<LinePositionDto>,
    /// Games that followed the whole line.
    pub games: u32,
    pub white_wins: u32,
    pub draws: u32,
    pub black_wins: u32,
    /// Score for the side to move at the position on the board, 0…1.
    pub score: f64,
    pub last_year: Option<u16>,
    /// Average Elo of both players.
    pub avg_elo: Option<u32>,
}

/// The main line and the lines from a position.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LinesDto {
    pub fen: String,
    /// Games whose moves were counted.
    pub games: u32,
    pub main_line: Option<ReferenceLineDto>,
    pub lines: Vec<ReferenceLineDto>,
}

/// The lines, or the job counting them (ask again when it's done).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ReferenceLinesDto {
    Ready {
        lines: Box<LinesDto>,
    },
    Job {
        #[ts(type = "number")]
        job: u64,
        /// Games it reads.
        games: u32,
    },
}

/// A game's main line for the Reference tab's mini board.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameLineDto {
    pub id: u32,
    pub white: String,
    pub white_elo: Option<u16>,
    pub black: String,
    pub black_elo: Option<u16>,
    pub result: String,
    pub year: Option<u16>,
    pub event: String,
    /// The start position, then one per move.
    pub positions: Vec<LinePositionDto>,
    /// The first of `positions` that is the position on the board (0 when
    /// the game's main line doesn't reach it).
    pub start: u32,
}

/// Which name table: players, events (tournaments), sites or annotators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NameKindDto {
    Player,
    Event,
    Site,
    Annotator,
}

impl From<NameKindDto> for chess_db::NameKind {
    fn from(k: NameKindDto) -> chess_db::NameKind {
        match k {
            NameKindDto::Player => chess_db::NameKind::Player,
            NameKindDto::Event => chess_db::NameKind::Event,
            NameKindDto::Site => chess_db::NameKind::Site,
            NameKindDto::Annotator => chess_db::NameKind::Annotator,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct NameRowDto {
    pub id: u32,
    pub name: String,
    pub games: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NameListDto {
    /// Names that match (with at least one game).
    pub total: u32,
    /// The first of them, in the requested order.
    pub rows: Vec<NameRowDto>,
}

/// Result of checking a FEN: the normalized FEN, or `error` in plain words.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct FenCheckDto {
    pub fen: String,
    pub error: Option<String>,
}

/// Where a pasted FEN leads (QOL-10 "go to position").
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum GotoPositionDto {
    /// The game on screen reaches the position; its cursor moved there.
    InGame { view: Box<GameViewDto> },
    /// The game doesn't reach it: start a new game from `fen` (normalized).
    NewGame { fen: String },
    /// The text has no FEN in it.
    NoFen,
    /// The FEN isn't a legal position; `error` says why in plain words.
    Invalid { fen: String, error: String },
}

/// A PGN game pasted from the clipboard (IO-08 "paste game").
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum PastedGameDto {
    /// The text's first game, opened as a new unsaved game: in the session
    /// asked for unless it had unsaved changes, else in a new session
    /// (`view.session` says which).
    Game {
        view: Box<GameViewDto>,
        /// Games in the text (the first one is the one opened).
        games: u32,
        /// Problems in the opened game, as an import would list them.
        warnings: Vec<ImportWarningDto>,
    },
    /// The text holds no game: no moves and no tags. `reason` says why a
    /// game in it couldn't be read, if it had one (an unsupported variant,
    /// a bad FEN tag).
    NoGame { reason: Option<String> },
}

/// A problem found in one game while importing.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportWarningDto {
    /// One-based number of the game in the file.
    pub number: u32,
    /// Id the game got in the database; `None` if it was skipped.
    pub game: Option<u32>,
    /// `White – Black`, or the event when the players are unknown; empty
    /// for a skipped game.
    pub label: String,
    pub message: String,
}

/// The last import into a database, for the import warnings dialog.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportReportDto {
    pub file: String,
    #[ts(type = "number")]
    pub imported: u64,
    #[ts(type = "number")]
    pub skipped: u64,
    #[ts(type = "number")]
    pub warning_count: u64,
    /// The first warnings (up to 200 of `warning_count`).
    pub warnings: Vec<ImportWarningDto>,
}

impl ImportReportDto {
    pub fn new(file: String, s: &chess_db::ImportSummary) -> ImportReportDto {
        ImportReportDto {
            file,
            imported: s.imported,
            skipped: s.skipped,
            warning_count: s.warning_count,
            warnings: s
                .warnings
                .iter()
                .map(|w| ImportWarningDto {
                    number: w.game_index as u32 + 1,
                    game: w.game,
                    label: w.label.clone(),
                    message: w.message.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SavedFilter;

    #[test]
    fn a_saved_view_fills_the_filter_box_and_the_search_dialog() {
        let q = |text: &str, white: &str| HeaderQueryDto {
            text: text.into(),
            white: white.into(),
            ..Default::default()
        };
        let view = |steps: Vec<FilterStep>| DbView {
            filter: Some(SavedFilter { steps, description: "d".into() }),
            ..DbView::default()
        };
        let dto = DbViewDto::from(&view(vec![FilterStep::Header { query: q("carlsen", ""), quick: true }]));
        assert_eq!(
            (dto.quick_text.as_str(), dto.last_query, dto.filter.as_deref()),
            ("carlsen", None, Some("d"))
        );

        // A search within the quick filter's results: the box stays empty.
        let dto = DbViewDto::from(&view(vec![
            FilterStep::Header { query: q("carlsen", ""), quick: true },
            FilterStep::Header { query: q("", "Carlsen"), quick: false },
        ]));
        assert_eq!((dto.quick_text.as_str(), dto.last_query), ("", Some(q("", "Carlsen"))));
        assert_eq!(DbViewDto::from(&DbView::default()), DbViewDto::default());
    }
}
