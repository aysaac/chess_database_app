//! Errors as the API returns them: a status and
//! `{"error": {"code": "...", "message": "..."}}`.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chess_db::DbError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    /// Machine-readable, e.g. `notFound`.
    pub code: &'static str,
    pub message: String,
    /// Seconds, for `429 Too Many Requests`.
    pub retry_after: Option<u64>,
}

pub type ApiResult<T> = Result<T, ApiError>;

/// The body of an error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError { status, code, message: message.into(), retry_after: None }
    }

    /// Also what a database the caller may not see gives, so its existence
    /// isn't revealed.
    pub fn not_found() -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, "notFound", "Not found")
    }

    pub fn bad_request(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, "badRequest", message)
    }

    pub fn unauthorized() -> ApiError {
        ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "Log in first")
    }

    pub fn forbidden(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn conflict(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn too_large(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "tooLarge", message)
    }

    pub fn too_many(retry_after: u64) -> ApiError {
        ApiError {
            retry_after: Some(retry_after),
            ..ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "tooManyAttempts",
                format!("Too many failed attempts: try again in {} minutes", retry_after.div_ceil(60)),
            )
        }
    }

    /// Logs the cause and returns a generic error: causes can name paths
    /// on the server.
    pub fn internal(cause: impl std::fmt::Display) -> ApiError {
        log::error!("internal error: {cause}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Internal error (see the server log)")
    }
}

impl From<DbError> for ApiError {
    fn from(e: DbError) -> ApiError {
        match e {
            DbError::NotFound(_) => ApiError::not_found(),
            DbError::Invalid(m) => ApiError::bad_request(m),
            DbError::Locked(_) => ApiError::conflict("The database is busy"),
            DbError::Cancelled => ApiError::conflict("Cancelled"),
            other => ApiError::internal(other),
        }
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> ApiError {
        ApiError::internal(format!("metadata store: {e}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> ApiError {
        ApiError::internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: ErrorDetail { code: self.code.to_owned(), message: self.message } };
        let mut response = (self.status, axum::Json(body)).into_response();
        if let Some(secs) = self.retry_after {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        response
    }
}
