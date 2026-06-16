use tower_http::services::ServeDir;

pub fn service() -> ServeDir {
    ServeDir::new("frontend/dist")
}
