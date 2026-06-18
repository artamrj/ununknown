use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::fmt;

#[derive(Debug)]
pub enum ApiError {
    Validation(String),
    NotFound(String),
    Conflict(String),
    Provider(String),
    Io(String),
    Forbidden(String),
    Timeout(String),
    Internal(anyhow::Error),
}

impl ApiError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn provider(message: impl Into<String>) -> Self {
        Self::Provider(message.into())
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::Io(message.into())
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
    }

    pub fn internal(error: impl Into<anyhow::Error>) -> Self {
        Self::Internal(error.into())
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Provider(_) => StatusCode::BAD_GATEWAY,
            Self::Io(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::Provider(message)
            | Self::Io(message)
            | Self::Forbidden(message)
            | Self::Timeout(message) => f.write_str(message),
            Self::Internal(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self::Internal(value)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(value: sqlx::Error) -> Self {
        match value {
            sqlx::Error::RowNotFound => Self::NotFound("resource not found".into()),
            error => Self::Internal(error.into()),
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self::Internal(value.into())
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(value: tokio::task::JoinError) -> Self {
        Self::Internal(value.into())
    }
}

impl From<tokio::sync::AcquireError> for ApiError {
    fn from(value: tokio::sync::AcquireError) -> Self {
        Self::Internal(value.into())
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(value: reqwest::Error) -> Self {
        if value.is_timeout() {
            Self::Timeout(value.to_string())
        } else {
            Self::Provider(value.to_string())
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status(),
            Json(serde_json::json!({"error":self.to_string()})),
        )
            .into_response()
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn maps_variants_to_status_codes() {
        let cases = [
            (
                ApiError::validation("invalid settings"),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (ApiError::not_found("missing track"), StatusCode::NOT_FOUND),
            (ApiError::conflict("already scanning"), StatusCode::CONFLICT),
            (
                ApiError::provider("provider unavailable"),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ApiError::timeout("provider timed out"),
                StatusCode::GATEWAY_TIMEOUT,
            ),
            (ApiError::forbidden("secret issue"), StatusCode::FORBIDDEN),
            (
                ApiError::io("disk failed"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                ApiError::internal(anyhow::anyhow!("bug")),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (error, status) in cases {
            assert_eq!(error.into_response().status(), status);
        }
    }
}
