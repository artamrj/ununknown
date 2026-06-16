use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub struct ApiError(pub anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(value: E) -> Self {
        Self(value.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":self.0.to_string()})),
        )
            .into_response()
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
