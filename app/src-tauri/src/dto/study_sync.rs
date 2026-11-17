//! What the study sync commands (NET-13, the Sync button) send and take:
//! the database's links, a sync's preview and outcome, a game waiting for
//! the user.

use chess_db::SyncMode;
use chess_net::study::sync::{PendingKind, Step};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::lichess::StudyVisibilityDto;

/// How a linked database follows its study.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SyncModeDto {
    /// What changes on Lichess comes here; nothing goes back.
    Follow,
    /// Changes go both ways.
    TwoWay,
}

impl From<SyncMode> for SyncModeDto {
    fn from(m: SyncMode) -> SyncModeDto {
        match m {
            SyncMode::Follow => SyncModeDto::Follow,
            SyncMode::TwoWay => SyncModeDto::TwoWay,
        }
    }
}

impl From<SyncModeDto> for SyncMode {
    fn from(m: SyncModeDto) -> SyncMode {
        match m {
            SyncModeDto::Follow => SyncMode::Follow,
            SyncModeDto::TwoWay => SyncMode::TwoWay,
        }
    }
}

/// Why a game waits for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PendingKindDto {
    /// Changed here and on Lichess in ways that don't merge.
    Both,
    /// Its chapter was removed on Lichess, and it changed here.
    RemovedThere,
    /// The game is the chapter but differs from it, and they were never synced.
    NoBase,
}

impl From<PendingKind> for PendingKindDto {
    fn from(k: PendingKind) -> PendingKindDto {
        match k {
            PendingKind::Both => PendingKindDto::Both,
            PendingKind::RemovedThere => PendingKindDto::RemovedThere,
            PendingKind::NoBase => PendingKindDto::NoBase,
        }
    }
}

/// A game waiting for the user, in a list.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PendingGameDto {
    /// Its permanent id, to resolve it by.
    pub uid: String,
    /// Its place in the list now.
    pub game: u32,
    pub name: String,
    pub kind: PendingKindDto,
}

/// A study the database is linked to.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LinkedStudyDto {
    pub study: String,
    /// From a linked game's StudyName, if one has it.
    pub name: Option<String>,
    /// Its page on Lichess.
    pub url: String,
    pub mode: SyncModeDto,
    pub games: u32,
    pub pending: Vec<PendingGameDto>,
}

/// The study a static copy came from (IO-10's import), to link it to.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CopiedStudyDto {
    pub study: String,
    pub name: Option<String>,
    pub link: String,
}

/// The open database's links to studies.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StudySyncStatusDto {
    /// The studies it's linked to, the one with the most games first.
    pub studies: Vec<LinkedStudyDto>,
    /// Unlinked: the study its games say they came from, if they do.
    pub copied_from: Option<CopiedStudyDto>,
}

/// What to sync with.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SyncTargetDto {
    /// A study, by its link or id: the one the database is linked to, or one
    /// to link it to.
    Study { study: String, mode: SyncModeDto },
    /// A new study, made for the database's games.
    New { name: String, visibility: StudyVisibilityDto, mode: SyncModeDto },
}

/// What a sync does with a game or chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SyncStepDto {
    Unchanged,
    Pull,
    Push,
    Merge,
    KeptHere,
    Conflict,
    Pending,
    NewThere,
    Adopt,
    NewHere,
    NoRoom,
    NotSent,
    RemovedThere,
    DeletedHere,
    DeleteChapter,
    Open,
}

impl From<Step> for SyncStepDto {
    fn from(s: Step) -> SyncStepDto {
        match s {
            Step::Unchanged => SyncStepDto::Unchanged,
            Step::Pull => SyncStepDto::Pull,
            Step::Push => SyncStepDto::Push,
            Step::Merge => SyncStepDto::Merge,
            Step::KeptHere => SyncStepDto::KeptHere,
            Step::Conflict => SyncStepDto::Conflict,
            Step::Pending => SyncStepDto::Pending,
            Step::NewThere => SyncStepDto::NewThere,
            Step::Adopt => SyncStepDto::Adopt,
            Step::NewHere => SyncStepDto::NewHere,
            Step::NoRoom => SyncStepDto::NoRoom,
            Step::NotSent => SyncStepDto::NotSent,
            Step::RemovedThere => SyncStepDto::RemovedThere,
            Step::DeletedHere => SyncStepDto::DeletedHere,
            Step::DeleteChapter => SyncStepDto::DeleteChapter,
            Step::Open => SyncStepDto::Open,
        }
    }
}

/// A game or chapter in a preview, or one an apply left as it was.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncItemDto {
    pub name: String,
    pub step: SyncStepDto,
    pub note: Option<String>,
}

/// How many games or chapters each step is for.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncCountDto {
    pub step: SyncStepDto,
    pub count: u32,
}

/// What a sync would do: the preview, before the one confirm.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncPreviewDto {
    /// None for a study the sync makes.
    pub study: Option<String>,
    pub name: String,
    pub url: Option<String>,
    pub mode: SyncModeDto,
    /// Each step there's something for, Unchanged too.
    pub counts: Vec<SyncCountDto>,
    /// The games and chapters that change or need a word, at most 300.
    pub items: Vec<SyncItemDto>,
    /// Chapters the study has room for after the new ones.
    pub room: u32,
    /// Games here outside the study not looked at (following only).
    pub unlisted: u32,
    /// Nothing would change.
    pub nothing: bool,
}

/// What a sync did.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncReportDto {
    pub study: String,
    pub name: String,
    pub url: String,
    pub made: bool,
    pub counts: Vec<SyncCountDto>,
    /// Games left as they were, and why.
    pub skipped: Vec<SyncItemDto>,
    /// Why it stopped before the end.
    pub stopped: Option<String>,
    pub cancelled: bool,
}

/// One thing both sides changed differently.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncQuestionDto {
    /// "12...Nf6: comment".
    pub what: String,
    pub mine: String,
    pub theirs: String,
}

/// A side of a conflict, in short.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncSideDto {
    /// Its main line in SAN.
    pub line: String,
    pub moves: u32,
    pub comments: u32,
}

/// A game waiting for the user, for the resolve dialog.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncConflictDto {
    pub uid: String,
    pub game: u32,
    pub name: String,
    pub study: String,
    /// The chapter's page on Lichess.
    pub url: String,
    pub kind: PendingKindDto,
    pub two_way: bool,
    /// When it was found, `YYYY-MM-DD`.
    pub found: String,
    pub questions: Vec<SyncQuestionDto>,
    pub mine: SyncSideDto,
    /// None: the chapter was removed on Lichess.
    pub theirs: Option<SyncSideDto>,
}

/// How the user settles a game waiting for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SyncChoiceDto {
    Mine,
    Theirs,
    Both,
}
