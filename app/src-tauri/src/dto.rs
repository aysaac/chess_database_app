//! Serializable types shared with the frontend. TypeScript definitions are
//! generated into `app/src/lib/bindings/` by `npm run bindings`.

use std::collections::HashMap;

use chess_core::game::{GameResult, Headers, Shape, ShapeColor};
use chess_core::shakmaty::Square;
use chess_db::{DbInfo, EloMode, GameRow, HeaderQuery, ResultFilter, SortKey, SortSpec};
use chess_jobs::{JobState, JobStatus};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;

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

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FilterDto {
    pub count: u32,
    pub description: String,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ResultFilterDto {
    WhiteWins,
    BlackWins,
    Draw,
    Decisive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EloModeDto {
    #[default]
    Both,
    Either,
    Average,
}

/// Header search criteria (SRCH-01); empty fields match everything.
#[derive(Debug, Clone, Default, Deserialize, TS)]
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
    /// The position is deeper than the indexed plies: games reaching it
    /// later than that are missing (a full search finds them).
    pub beyond_index: bool,
    pub max_ply: u16,
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
