use chess_db::DbError;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ErrorCode {
    NotFound,
    Locked,
    NotSorted,
    AlreadyExists,
    NotADatabase,
    InvalidInput,
    Io,
    Corrupt,
    /// An engine failed to start or stopped responding.
    Engine,
    /// An online server (NET-01) couldn't be reached or failed.
    Network,
    Internal,
}

/// Error returned to the frontend by every command.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> AppError {
        AppError { code, message: message.into() }
    }

    pub fn invalid(message: impl Into<String>) -> AppError {
        AppError::new(ErrorCode::InvalidInput, message)
    }

    pub fn not_found(what: impl std::fmt::Display) -> AppError {
        AppError::new(ErrorCode::NotFound, format!("{what} not found"))
    }
}

impl From<DbError> for AppError {
    fn from(e: DbError) -> AppError {
        let code = match &e {
            DbError::Io(_) => ErrorCode::Io,
            DbError::Locked(_) => ErrorCode::Locked,
            DbError::NotADatabase(_) => ErrorCode::NotADatabase,
            DbError::AlreadyExists(_) => ErrorCode::AlreadyExists,
            DbError::UnsupportedVersion(_) | DbError::Corrupt(_) => ErrorCode::Corrupt,
            DbError::NotFound(_) => ErrorCode::NotFound,
            DbError::NotSorted => ErrorCode::NotSorted,
            DbError::Cancelled => ErrorCode::Internal,
            DbError::Invalid(_) => ErrorCode::InvalidInput,
        };
        AppError::new(code, e.to_string())
    }
}

impl From<chess_engine::EngineError> for AppError {
    fn from(e: chess_engine::EngineError) -> AppError {
        AppError::new(ErrorCode::Engine, e.to_string())
    }
}

impl From<chess_core::Error> for AppError {
    fn from(e: chess_core::Error) -> AppError {
        AppError::invalid(e.to_string())
    }
}

impl From<chess_diagram::DiagramError> for AppError {
    fn from(e: chess_diagram::DiagramError) -> AppError {
        let code = match &e {
            chess_diagram::DiagramError::BadColor(_) | chess_diagram::DiagramError::BadSize(_) => {
                ErrorCode::InvalidInput
            }
            chess_diagram::DiagramError::Render(_) => ErrorCode::Internal,
        };
        AppError::new(code, e.to_string())
    }
}

impl From<chess_net::NetError> for AppError {
    fn from(e: chess_net::NetError) -> AppError {
        use chess_net::NetError;
        let code = match e {
            NetError::Db(e) => return AppError::from(e),
            NetError::BadUsername(_) => ErrorCode::InvalidInput,
            NetError::NoSuchPlayer { .. } => ErrorCode::NotFound,
            NetError::Cancelled => ErrorCode::Internal,
            NetError::RateLimited(_)
            | NetError::Unreachable { .. }
            | NetError::Status { .. }
            | NetError::BadData { .. }
            | NetError::Stalled(_) => ErrorCode::Network,
        };
        AppError::new(code, e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
