use std::sync::Arc;

use axum::{extract::{Request, State}, middleware::Next, response::Response};
use sea_orm::DatabaseConnection;

use crate::error::AppError;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub api_key: Option<Arc<str>>,
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Optional shared-secret auth: enforced only when API_KEY is configured.
pub async fn require_api_key(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(key) = &state.api_key {
        let ok = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|v| constant_time_eq(v.as_bytes(), key.as_bytes()))
            .unwrap_or(false);
        if !ok {
            return Err(AppError::Unauthorized);
        }
    }
    Ok(next.run(req).await)
}
