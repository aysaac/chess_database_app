//! The JSON the API sends and takes.
//!
//! Where the desktop app has a type for the same thing
//! (`app/src-tauri/src/dto.rs`: `GameRowDto`, `GamePage`, `HeadersDto`,
//! `NotationDto`, `ReferenceDto`, `LinesDto`, `JobDto`…), the type here has
//! the same name and fields, serialized the same way (camelCase), so the
//! desktop client (NET-03 phase 2) can share them. This module depends only
//! on serde, `chess-core` and `chess-db`, so it can move into a crate both
//! use then; it's kept here until there is a second user.

use chess_core::game::{Game, Headers, NodeId};
use chess_core::notation::{self, san, uci};
use chess_core::openings;
use chess_core::position;
use chess_core::shakmaty::{Chess, Color, Move, Position};
use chess_db::{
    EloMode, GameRow, HeaderQuery, ImportSummary, Line, ReferenceLines, ReferenceOverview, ResultFilter,
    SortKey, SortSpec, TreeStats,
};
use chess_jobs::{JobState, JobStatus};
use serde::{Deserialize, Deserializer, Serialize};

// ---- accounts ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        }
    }
}

/// The user making the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeDto {
    pub id: i64,
    pub username: String,
    pub role: Role,
    /// For a browser session: the token state-changing requests send in
    /// `X-CSRF-Token`. None with an API token.
    pub csrf_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Whether the first admin still has to be made (see `POST /api/setup`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatusDto {
    pub needed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    /// The setup token the server printed to its log at start.
    pub token: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordChange {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRequest {
    pub name: String,
}

/// An API token just made: the only time `token` is shown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTokenDto {
    pub id: i64,
    pub name: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenDto {
    pub id: i64,
    pub name: String,
    /// Unix seconds.
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

// ---- databases -----------------------------------------------------------

/// Who can read a database besides its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Visibility {
    /// The owner only (not the admin either).
    Private,
    /// The owner and the users it's shared with.
    Shared,
    /// Every user can read it; its shares can still give write access.
    Server,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Private => "private",
            Visibility::Shared => "shared",
            Visibility::Server => "server",
        }
    }

    pub fn parse(s: &str) -> Option<Visibility> {
        match s {
            "private" => Some(Visibility::Private),
            "shared" => Some(Visibility::Shared),
            "server" => Some(Visibility::Server),
            _ => None,
        }
    }
}

/// What a share gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareAccess {
    Read,
    /// Read, and import games into it.
    Write,
}

impl ShareAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            ShareAccess::Read => "read",
            ShareAccess::Write => "write",
        }
    }

    pub fn parse(s: &str) -> Option<ShareAccess> {
        match s {
            "read" => Some(ShareAccess::Read),
            "write" => Some(ShareAccess::Write),
            _ => None,
        }
    }
}

/// What the caller may do with a database, least first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Access {
    Read,
    /// Read and import.
    Write,
    /// Also rename, delete, set the visibility and the shares.
    Owner,
}

/// A database the caller can read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseDto {
    /// Opaque; never a path.
    pub id: String,
    pub name: String,
    pub owner: String,
    pub visibility: Visibility,
    pub access: Access,
    pub games: u32,
    pub size_bytes: u64,
    pub quota_bytes: Option<u64>,
    pub created_at: i64,
    /// Only for one database's info (`GET /api/databases/{id}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexStatusDto>,
}

/// State of a database's position index (SRCH-04), as the app's
/// `IndexStatusDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatusDto {
    pub built: bool,
    /// Games `0..covered` are indexed.
    pub covered: u32,
    pub up_to_date: bool,
    pub max_ply: u16,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDatabase {
    pub name: String,
    /// Private when left out.
    #[serde(default)]
    pub visibility: Option<Visibility>,
}

/// What the owner changes; fields left out stay.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDatabase {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub visibility: Option<Visibility>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDto {
    pub username: String,
    pub access: ShareAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetShare {
    pub access: ShareAccess,
}

/// `?confirm=` for deleting a database: its name, as typed by the user.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Confirm {
    #[serde(default)]
    pub confirm: String,
}

// ---- games ---------------------------------------------------------------

/// A game list row, as the app's `GameRowDto`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// A page of the game list, as the app's `GamePage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GamePage {
    /// Games in the (filtered) list.
    pub total: u32,
    pub offset: u32,
    pub rows: Vec<GameRowDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

impl From<SortKeyDto> for SortKey {
    fn from(k: SortKeyDto) -> SortKey {
        match k {
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResultFilterDto {
    WhiteWins,
    BlackWins,
    Draw,
    Decisive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EloModeDto {
    #[default]
    Both,
    Either,
    Average,
}

/// `GET /api/databases/{id}/games`: a page of the list in a sort order,
/// limited to the games matching the header criteria (SRCH-01; the fields
/// of the app's `HeaderQueryDto`, empty ones match everything).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GamesQuery {
    pub offset: u32,
    /// At most [`MAX_PAGE`]; 50 when left out.
    pub limit: Option<u32>,
    /// Game id order when left out.
    pub sort: Option<SortKeyDto>,
    pub desc: bool,
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
}

/// Rows per page at most.
pub const MAX_PAGE: u32 = 500;

impl GamesQuery {
    pub fn sort_spec(&self) -> SortSpec {
        SortSpec::new(self.sort.map_or(SortKey::Id, SortKey::from), self.desc)
    }

    pub fn page_limit(&self) -> u32 {
        self.limit.unwrap_or(50).min(MAX_PAGE)
    }

    /// The header criteria; deleted games are always left out.
    pub fn header_query(&self) -> HeaderQuery {
        HeaderQuery {
            white: self.white.clone(),
            black: self.black.clone(),
            ignore_colors: self.ignore_colors,
            text: self.text.clone(),
            event: self.event.clone(),
            site: self.site.clone(),
            annotator: self.annotator.clone(),
            year_from: self.year_from,
            year_to: self.year_to,
            eco_from: self.eco_from.clone(),
            eco_to: self.eco_to.clone(),
            result: self.result.map(|r| match r {
                ResultFilterDto::WhiteWins => ResultFilter::WhiteWins,
                ResultFilterDto::BlackWins => ResultFilter::BlackWins,
                ResultFilterDto::Draw => ResultFilter::Draw,
                ResultFilterDto::Decisive => ResultFilter::Decisive,
            }),
            elo_min: self.elo_min,
            elo_max: self.elo_max,
            elo_mode: match self.elo_mode {
                EloModeDto::Both => EloMode::Both,
                EloModeDto::Either => EloMode::Either,
                EloModeDto::Average => EloMode::Average,
            },
            moves_min: self.moves_min,
            moves_max: self.moves_max,
            annotated_only: self.annotated_only,
            skip_deleted: true,
        }
    }
}

/// A game's headers, as the app's `HeadersDto`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// A move of the notation, as the app's `NotationMoveDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotationMoveDto {
    pub id: u32,
    pub san: String,
    /// The move in UCI (castling as `e1g1`).
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

/// A game's move tree, as the app's `NotationDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotationDto {
    pub root_comment: String,
    pub moves: Vec<NotationMoveDto>,
}

fn command<'a>(game: &'a Game, id: NodeId, name: &str) -> Option<&'a str> {
    game.node(id).commands.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// The line starting at `first` (a move played from `pos`), with the
/// alternatives to each main-continuation move attached to it (as the
/// app's `view::notation`).
fn line(game: &Game, first: NodeId, mut pos: Chess) -> Vec<NotationMoveDto> {
    let mut out = Vec::new();
    let mut id = first;
    loop {
        let node = game.node(id);
        let mv = node.mv.expect("non-root node has a move");
        let parent = node.parent.expect("non-root node has a parent");
        let siblings = &game.node(parent).children;
        let variations = if siblings.first() == Some(&id) {
            siblings[1..].iter().map(|&alt| line(game, alt, pos.clone())).collect()
        } else {
            Vec::new()
        };
        out.push(NotationMoveDto {
            id: id.0,
            san: notation::san(&pos, mv),
            uci: notation::uci(mv),
            move_number: pos.fullmoves().get(),
            white: pos.turn() == Color::White,
            nags: node.nags.iter().map(|n| n.0).collect(),
            comment_before: node.comment_before.clone(),
            comment_after: node.comment_after.clone(),
            clock: command(game, id, "clk").map(str::to_owned),
            eval: command(game, id, "eval").map(str::to_owned),
            variations,
        });
        pos.play_unchecked(mv);
        match node.children.first() {
            Some(&next) => id = next,
            None => return out,
        }
    }
}

pub fn notation_of(game: &Game) -> NotationDto {
    let root = game.root();
    NotationDto {
        root_comment: root.comment_after.clone(),
        moves: root
            .children
            .first()
            .map(|&first| line(game, first, game.start_position()))
            .unwrap_or_default(),
    }
}

/// One game: its row, headers, PGN and move tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameDto {
    pub id: u32,
    pub row: GameRowDto,
    pub headers: HeadersDto,
    /// The set-up position it starts from, if any.
    pub start_fen: Option<String>,
    pub pgn: String,
    pub notation: NotationDto,
    pub opening: Option<OpeningDto>,
}

// ---- the position tree and the Reference tab (OPEN-14) -------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpeningDto {
    pub eco: String,
    pub name: String,
}

impl From<&openings::Opening> for OpeningDto {
    fn from(o: &openings::Opening) -> OpeningDto {
        OpeningDto { eco: o.eco.to_owned(), name: o.name.to_owned() }
    }
}

/// `?fen=` for the tree and the lines; the standard start when left out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FenQuery {
    #[serde(default)]
    pub fen: Option<String>,
}

/// One row of the opening tree, as the app's `TreeRowDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrendDto {
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotDto {
    pub recent: u32,
    pub share: f64,
    pub all_share: f64,
    pub trend: Option<TrendDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BestPlayerDto {
    pub name: String,
    pub surname: String,
    pub elo: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceMoveDto {
    pub row: TreeRowDto,
    pub hot: Option<HotDto>,
    pub best: Vec<BestPlayerDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// The position tree at a position with its top games, as the app's
/// `ReferenceDto` (`chess_db::reference_overview`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceDto {
    pub fen: String,
    pub opening: Option<OpeningDto>,
    pub total: TreeRowDto,
    pub moves: Vec<ReferenceMoveDto>,
    pub recent_from: Option<u16>,
    pub recent_to: Option<u16>,
    /// Strongest first (average Elo of both players), at most 100.
    pub top_games: Vec<TopGameDto>,
    /// The index has data.
    pub indexed: bool,
    /// Some new games couldn't be included (an index update is running).
    pub partial: bool,
    /// Games may reach the position past the indexed plies.
    pub beyond_index: bool,
    pub max_ply: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinePositionDto {
    pub fen: String,
    pub san: Option<String>,
    pub uci: Option<String>,
    pub ply: u32,
    pub move_number: u32,
    pub white: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceLineDto {
    pub positions: Vec<LinePositionDto>,
    pub games: u32,
    pub white_wins: u32,
    pub draws: u32,
    pub black_wins: u32,
    pub score: f64,
    pub last_year: Option<u16>,
    pub avg_elo: Option<u32>,
}

/// The main line and lines from a position, as the app's `LinesDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinesDto {
    pub fen: String,
    pub games: u32,
    pub main_line: Option<ReferenceLineDto>,
    pub lines: Vec<ReferenceLineDto>,
}

/// The lines, or the job counting them (ask again when it's done), as the
/// app's `ReferenceLinesDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReferenceLinesDto {
    Ready {
        lines: Box<LinesDto>,
    },
    Job {
        job: u64,
        /// Games it reads.
        games: u32,
    },
}

/// Score for the side to move, 0…1 (0.5 when no game was decided).
pub fn side_score(white_wins: u32, draws: u32, black_wins: u32, white_to_move: bool) -> f64 {
    let (wins, losses) = if white_to_move { (white_wins, black_wins) } else { (black_wins, white_wins) };
    let decided = wins + losses + draws;
    if decided == 0 { 0.5 } else { (f64::from(wins) + 0.5 * f64::from(draws)) / f64::from(decided) }
}

/// As the app's `tree_row`.
pub fn tree_row(stats: &TreeStats, white_to_move: bool, mv: Option<(String, String)>) -> TreeRowDto {
    let decided = stats.white_wins + stats.black_wins + stats.draws;
    let score = side_score(stats.white_wins, stats.draws, stats.black_wins, white_to_move);
    let avg = |sum: u64, n: u32| (n > 0).then(|| (sum / u64::from(n)) as u32);
    let opp = avg(stats.opp_elo_sum, stats.opp_elo_count);
    let performance = opp.filter(|_| decided > 0).map(|o| {
        let p = score.clamp(0.01, 0.99);
        o as i32 + (400.0 * (p / (1.0 - p)).log10()).round().clamp(-800.0, 800.0) as i32
    });
    TreeRowDto {
        uci: mv.as_ref().map(|(u, _)| u.clone()),
        san: mv.map(|(_, s)| s),
        games: stats.games,
        white_wins: stats.white_wins,
        draws: stats.draws,
        black_wins: stats.black_wins,
        score,
        avg_elo: avg(stats.elo_sum, stats.elo_count),
        performance,
        last_year: (stats.last_year > 0).then_some(stats.last_year),
    }
}

/// Plies from the standard start to `pos` (by move number).
fn ply_of(pos: &Chess) -> u32 {
    (pos.fullmoves().get() - 1) * 2 + u32::from(pos.turn().is_black())
}

fn surname(name: &str) -> String {
    name.split(',').next().unwrap_or(name).trim().to_owned()
}

fn year_of(date: &str) -> Option<u16> {
    date.get(..4).and_then(|y| y.parse().ok()).filter(|&y| y > 0)
}

/// The Reference overview at `pos`, as the app builds its `ReferenceDto`.
/// A FEN carries no game, so `beyondIndex` comes from the move number alone.
pub fn reference_dto(pos: &Chess, o: ReferenceOverview, indexed: bool) -> ReferenceDto {
    let white = pos.turn().is_white();
    let moves = o
        .moves
        .iter()
        .map(|m| ReferenceMoveDto {
            row: tree_row(&m.stats, white, m.mv.map(|mv| (uci(mv), san(pos, mv)))),
            hot: m.hot.as_ref().map(|h| HotDto {
                recent: h.recent,
                share: h.share,
                all_share: h.all_share,
                trend: h.trend.map(|t| match t {
                    chess_db::Trend::Up => TrendDto::Up,
                    chess_db::Trend::Down => TrendDto::Down,
                }),
            }),
            best: m
                .best
                .iter()
                .map(|p| BestPlayerDto { name: p.name.clone(), surname: surname(&p.name), elo: p.elo })
                .collect(),
        })
        .collect();
    let top_games = o
        .top_games
        .into_iter()
        .map(|g| TopGameDto {
            id: g.row.id,
            year: year_of(&g.row.date),
            white: g.row.white,
            white_elo: g.row.white_elo,
            black: g.row.black,
            black_elo: g.row.black_elo,
            result: g.row.result.as_str().to_owned(),
            event: g.row.event,
            ply: g.ply,
        })
        .collect();
    ReferenceDto {
        fen: position::to_fen(pos),
        opening: openings::at_position(pos).map(OpeningDto::from),
        total: tree_row(&o.total, white, None),
        moves,
        recent_from: o.recent_years.map(|(from, _)| from),
        recent_to: o.recent_years.map(|(_, to)| to),
        top_games,
        indexed,
        partial: o.partial,
        beyond_index: ply_of(pos) > u32::from(o.max_ply),
        max_ply: o.max_ply,
    }
}

fn positions(start: &Chess, moves: &[Move]) -> Vec<LinePositionDto> {
    let mut pos = start.clone();
    let mut out = Vec::with_capacity(moves.len() + 1);
    out.push(LinePositionDto {
        fen: position::to_fen(&pos),
        san: None,
        uci: None,
        ply: 0,
        move_number: 0,
        white: false,
    });
    for (i, &m) in moves.iter().enumerate() {
        let (move_number, white) = (pos.fullmoves().get(), pos.turn().is_white());
        let san = san(&pos, m);
        pos.play_unchecked(m);
        out.push(LinePositionDto {
            fen: position::to_fen(&pos),
            san: Some(san),
            uci: Some(uci(m)),
            ply: i as u32 + 1,
            move_number,
            white,
        });
    }
    out
}

fn line_dto(pos: &Chess, line: &Line) -> ReferenceLineDto {
    let s = &line.stats;
    ReferenceLineDto {
        positions: positions(pos, &line.moves),
        games: s.games,
        white_wins: s.white_wins,
        draws: s.draws,
        black_wins: s.black_wins,
        score: side_score(s.white_wins, s.draws, s.black_wins, pos.turn().is_white()),
        last_year: (s.last_year > 0).then_some(s.last_year),
        avg_elo: (s.elo_count > 0).then(|| (s.elo_sum / u64::from(s.elo_count)) as u32),
    }
}

pub fn lines_dto(pos: &Chess, lines: &ReferenceLines) -> LinesDto {
    LinesDto {
        fen: position::to_fen(pos),
        games: lines.games,
        main_line: lines.main_line.as_ref().map(|l| line_dto(pos, l)),
        lines: lines.lines.iter().map(|l| line_dto(pos, l)).collect(),
    }
}

// ---- jobs and imports ------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStateDto {
    Running,
    Done,
    Failed,
    Cancelled,
}

/// A background job, as the app's `JobDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDto {
    pub id: u64,
    pub kind: String,
    pub label: String,
    pub state: JobStateDto,
    pub done: u64,
    pub total: u64,
    pub message: String,
    pub error: Option<String>,
}

pub fn job_state(s: &JobState) -> (JobStateDto, Option<String>) {
    match s {
        JobState::Running => (JobStateDto::Running, None),
        JobState::Done => (JobStateDto::Done, None),
        JobState::Failed(e) => (JobStateDto::Failed, Some(e.clone())),
        JobState::Cancelled => (JobStateDto::Cancelled, None),
    }
}

impl From<&JobStatus> for JobDto {
    fn from(s: &JobStatus) -> JobDto {
        let (state, error) = job_state(&s.state);
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

/// A job, for a user who may read its database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfoDto {
    #[serde(flatten)]
    pub job: JobDto,
    /// The database it works on.
    pub database: String,
    /// An import's summary, once it's done; for whoever started it and the
    /// database's owner.
    pub report: Option<ImportReportDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportWarningDto {
    /// One-based number of the game in the file.
    pub number: u32,
    pub game: Option<u32>,
    pub label: String,
    pub message: String,
}

/// What an import did, as the app's `ImportReportDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReportDto {
    pub file: String,
    pub imported: u64,
    pub skipped: u64,
    pub warning_count: u64,
    pub warnings: Vec<ImportWarningDto>,
}

impl ImportReportDto {
    pub fn new(file: String, s: &ImportSummary) -> ImportReportDto {
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

/// An import started: poll `GET /api/jobs/{job}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStarted {
    pub job: u64,
}

/// `?filename=` for an upload: shown in the job and its report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UploadQuery {
    #[serde(default)]
    pub filename: Option<String>,
}

/// A file of the server's import folder, relative to it with `/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFileRequest {
    pub path: String,
}

// ---- admin ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub disabled: bool,
    /// Most the user's databases may take together.
    pub quota_bytes: Option<u64>,
    pub created_at: i64,
    /// Databases they own, and their size.
    pub databases: u32,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUser {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub role: Option<Role>,
}

/// Fields left out stay; `"quotaBytes": null` removes the quota.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateUser {
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default, deserialize_with = "some", skip_serializing_if = "Option::is_none")]
    pub quota_bytes: Option<Option<u64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPassword {
    pub password: String,
}

/// A database as the admin sees it: metadata only, never games.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminDatabaseDto {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub owner_id: i64,
    pub visibility: Visibility,
    pub games: u32,
    pub size_bytes: u64,
    pub quota_bytes: Option<u64>,
    pub created_at: i64,
}

/// The only thing the admin changes on a database that isn't theirs: the
/// quota. Visibility, shares and owner aren't fields here, and unknown
/// fields are refused.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminUpdateDatabase {
    #[serde(default, deserialize_with = "some", skip_serializing_if = "Option::is_none")]
    pub quota_bytes: Option<Option<u64>>,
}

/// A job as the admin sees it: what runs where, not what it found.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminJobDto {
    pub id: u64,
    pub kind: String,
    pub state: JobStateDto,
    pub done: u64,
    pub total: u64,
    pub database: String,
    pub database_name: String,
    pub started_by: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskDto {
    pub data_dir: String,
    /// Of the file system holding the data directory (unknown off Unix).
    pub fs_total_bytes: Option<u64>,
    pub fs_free_bytes: Option<u64>,
    pub databases: u32,
    pub databases_bytes: u64,
    pub metadata_bytes: u64,
    pub uploads_bytes: u64,
    pub import_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportKind {
    Pgn,
    /// A ChessBase database (IO-03), by its `.cbh` file.
    Cbh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFileDto {
    pub path: String,
    pub size_bytes: u64,
    pub kind: ImportKind,
}

/// `Some(None)` for an explicit `null`, so a field can be cleared.
fn some<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_clears_and_absent_keeps() {
        let u: UpdateUser = serde_json::from_str(r#"{"quotaBytes": null}"#).unwrap();
        assert_eq!(u.quota_bytes, Some(None));
        let u: UpdateUser = serde_json::from_str(r#"{"quotaBytes": 5}"#).unwrap();
        assert_eq!(u.quota_bytes, Some(Some(5)));
        let u: UpdateUser = serde_json::from_str(r#"{"disabled": true}"#).unwrap();
        assert_eq!(u.quota_bytes, None);
        assert!(serde_json::from_str::<AdminUpdateDatabase>(r#"{"visibility": "server"}"#).is_err());
        assert!(serde_json::from_str::<AdminUpdateDatabase>(r#"{"owner": 1}"#).is_err());
    }

    #[test]
    fn query_defaults() {
        let q = GamesQuery::default();
        assert_eq!(q.page_limit(), 50);
        assert!(q.sort_spec().is_natural_order());
        assert_eq!(q.header_query(), HeaderQuery { skip_deleted: true, ..HeaderQuery::default() });
        let q = GamesQuery { limit: Some(10_000), ..GamesQuery::default() };
        assert_eq!(q.page_limit(), MAX_PAGE);
    }
}
