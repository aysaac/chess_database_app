//! What the Lichess commands send and take: the login (NET-11), a study to
//! import (IO-10), games out to studies (NET-12).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Who is logged in to Lichess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LichessAccountDto {
    /// `None`: nobody.
    pub username: Option<String>,
    /// The token is in the OS's credential store, so the login lasts after
    /// the app closes; otherwise it lasts until then.
    pub kept: bool,
}

impl LichessAccountDto {
    pub fn new(username: Option<&str>, kept: bool) -> LichessAccountDto {
        LichessAccountDto { username: username.map(str::to_owned), kept: kept && username.is_some() }
    }
}

/// What logging out did.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogoutDto {
    /// Lichess revoked the token.
    pub revoked: bool,
    /// Why it couldn't (the app forgot the token anyway).
    pub problem: Option<String>,
}

/// A study in a user's list.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StudyInfoDto {
    pub id: String,
    pub name: String,
    /// When it last changed, `YYYY-MM-DD`.
    pub updated: String,
    /// Its lichess.org link, to look it up or send games to it by.
    pub link: String,
}

/// A study (or one chapter of it) looked up, to import.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StudyPreviewDto {
    /// The study's (or the chapter's) lichess.org link, to import it by.
    pub link: String,
    /// Its page on the server, to open in the browser.
    pub url: String,
    pub name: String,
    /// One chapter of the study, not all of them.
    pub one_chapter: bool,
    /// The chapters' names, in order.
    pub chapters: Vec<String>,
    /// It took the login to see it (private or unlisted).
    pub private: bool,
}

/// Who can see a new study.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StudyVisibilityDto {
    Public,
    Unlisted,
    Private,
}

impl From<StudyVisibilityDto> for chess_net::study::Visibility {
    fn from(v: StudyVisibilityDto) -> chess_net::study::Visibility {
        use chess_net::study::Visibility;
        match v {
            StudyVisibilityDto::Public => Visibility::Public,
            StudyVisibilityDto::Unlisted => Visibility::Unlisted,
            StudyVisibilityDto::Private => Visibility::Private,
        }
    }
}

/// Where games go: new studies (one, or several of 64 chapters each), or a
/// study of the user's by its link.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum StudyTargetDto {
    New { name: String, visibility: StudyVisibilityDto },
    Existing { study: String },
}

/// A study games went to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SentStudyDto {
    pub name: String,
    pub url: String,
    /// Made for these games.
    pub made: bool,
    pub sent: u32,
    /// The chapters Lichess made of them.
    pub chapters: u32,
    /// What Lichess said about games it didn't take.
    pub refused: Option<String>,
}

/// What the last export to studies did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StudyExportReportDto {
    /// The games asked to be sent (deleted ones left out).
    pub games: u32,
    pub sent: u32,
    pub not_sent: u32,
    /// Games too long for Lichess, among those not sent.
    pub too_long: u32,
    pub studies: Vec<SentStudyDto>,
    /// Why it stopped before the last game, when it did.
    pub stopped: Option<String>,
    pub cancelled: bool,
}
