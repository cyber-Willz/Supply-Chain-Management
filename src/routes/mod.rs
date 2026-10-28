use axum::{extract::State, middleware, routing::get, Json, Router};
use chrono::Utc;
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    state::{require_api_key, AppState},
};

pub mod inventory;
pub mod master;
pub mod planning;
pub mod procurement;
pub mod purchasing;
pub mod quality;
pub mod reports;
pub mod sales;
pub mod supplier_perf;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(master::routes())
        .merge(inventory::routes())
        .merge(purchasing::routes())
        .merge(sales::routes())
        .merge(reports::routes())
        .merge(quality::routes())
        .merge(supplier_perf::routes())
        .merge(planning::routes())
        .merge(procurement::routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_api_key));

    Router::new()
        .route("/health", get(health))
        .nest("/api/v1", api)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}

async fn health(State(s): State<AppState>) -> Json<Value> {
    let db_ok = s
        .db
        .execute_unprepared("SELECT 1")
        .await
        .is_ok();
    Json(json!({ "status": if db_ok { "ok" } else { "degraded" }, "database": db_ok }))
}

// ---- shared helpers ------------------------------------------------------------------------

pub fn page_limit(limit: Option<u64>) -> u64 {
    limit.unwrap_or(50).clamp(1, 500)
}

/// Human-friendly document number, e.g. PO-20261025-A1B2C3.
pub fn gen_number(prefix: &str) -> String {
    let rand = Uuid::new_v4().simple().to_string()[..6].to_uppercase();
    format!("{}-{}-{}", prefix, Utc::now().format("%Y%m%d"), rand)
}

pub fn require_text(field: &str, value: &str) -> AppResult<String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(AppError::bad(format!("{field} must not be empty")));
    }
    Ok(v.to_string())
}

pub fn require_positive(field: &str, value: i64) -> AppResult<i64> {
    if value <= 0 {
        return Err(AppError::bad(format!("{field} must be greater than zero")));
    }
    Ok(value)
}

pub fn clean_lot(lot: &Option<String>) -> String {
    lot.as_deref().unwrap_or("").trim().to_string()
}
