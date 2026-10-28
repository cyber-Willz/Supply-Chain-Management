//! Planning analytics: statistical replenishment, ABC/XYZ classification, forecast accuracy.
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{NaiveDate, Utc};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
    stats::{norm_inv, safety_stock, service_level},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/reports/replenishment", get(replenishment))
        .route("/reports/abc-xyz", get(abc_xyz))
        .route("/reports/forecast-accuracy", get(forecast_accuracy))
        .route("/forecasts", post(upsert_forecasts))
}

/// Used when a product has fewer than MIN_LT_SAMPLES receipts: lead-time sd = 30% of the mean
/// (datasets show lead-time CVs of 0.23 aerospace, 0.47 retail, 0.63 defence).
const DEFAULT_LT_CV: f64 = 0.30;
const MIN_LT_SAMPLES: i64 = 5;

#[derive(Deserialize)]
pub struct ReplQuery {
    warehouse_id: Option<Uuid>,
    /// Demand history window in days (default 90, 14..=730)
    lookback_days: Option<i32>,
    /// Evaluate as of this date instead of today (backtests, imported history)
    as_of: Option<NaiveDate>,
}

#[derive(Debug, FromQueryResult)]
struct ReplRow {
    product_id: Uuid,
    sku: String,
    name: String,
    criticality_class: String,
    unit_cost: f64,
    holding_cost_rate: f64,
    stockout_cost: Option<f64>,
    review_period_days: i32,
    lead_time_days: i32,
    reorder_point: i64,
    reorder_qty: i64,
    mu: f64,
    sd: f64,
    mu30: f64,
    lt_n: Option<i64>,
    lt_mean: Option<f64>,
    lt_sd: Option<f64>,
    available: i64,
    on_order: i64,
}

#[derive(Serialize)]
pub struct ReplItem {
    product_id: Uuid,
    sku: String,
    name: String,
    criticality_class: String,
    daily_demand_mean: f64,
    daily_demand_sd: f64,
    /// last-4-weeks demand / window demand: >1.25 rising, <0.75 falling (seasonality check)
    recent_demand_ratio: Option<f64>,
    demand_trend: &'static str,
    lead_time_days: f64,
    lead_time_sd_days: f64,
    lead_time_source: &'static str,
    review_period_days: i32,
    service_level: f64,
    safety_stock: i64,
    recommended_reorder_point: i64,
    order_up_to_level: i64,
    current_reorder_point: i64,
    reorder_point_action: &'static str,
    available: i64,
    on_order: i64,
    inventory_position: i64,
    coverage_days: Option<f64>,
    reorder_now: bool,
    suggested_order_qty: i64,
}

/// Demand is measured in complete weeks (robust to lumpy posting days) and converted to daily:
/// mean/7, sd/sqrt(7).
/// Safety stock = z * sqrt((L+R)*sd_d^2 + mu_d^2*sd_L^2); reorder point = mu_d*(L+R) + SS;
/// service level from the critical ratio (stock-out vs holding cost) or the criticality class.
async fn replenishment(
    State(s): State<AppState>,
    Query(q): Query<ReplQuery>,
) -> AppResult<Json<Vec<ReplItem>>> {
    let lookback = q.lookback_days.unwrap_or(90);
    if !(14..=730).contains(&lookback) {
        return Err(AppError::bad("lookback_days must be between 14 and 730"));
    }
    let rows = ReplRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH weeks AS (
           SELECT generate_series(date_trunc('week', $3::date)::date - 7 * (($2::int + 6) / 7),
                                  date_trunc('week', $3::date)::date - 7, interval '1 week')::date AS w),
         iss AS (
           SELECT sm.product_id, date_trunc('week', sm.created_at)::date AS w, SUM(-sm.quantity_delta)::float8 AS q
           FROM stock_movements sm JOIN locations l ON l.id = sm.location_id
           WHERE sm.movement_type = 'issue'
             AND sm.created_at::date >= date_trunc('week', $3::date)::date - 7 * (($2::int + 6) / 7)
             AND sm.created_at::date < date_trunc('week', $3::date)::date
             AND ($1::uuid IS NULL OR l.warehouse_id = $1)
           GROUP BY 1, 2),
         dem AS (
           SELECT p.id AS product_id, (AVG(COALESCE(i.q, 0)) / 7.0)::float8 AS mu,
                  (COALESCE(STDDEV_SAMP(COALESCE(i.q, 0)), 0) / SQRT(7.0))::float8 AS sd,
                  (COALESCE(AVG(COALESCE(i.q, 0)) FILTER (WHERE weeks.w >= date_trunc('week', $3::date)::date - 28), 0) / 7.0)::float8 AS mu30
           FROM products p CROSS JOIN weeks LEFT JOIN iss i ON i.product_id = p.id AND i.w = weeks.w
           WHERE p.active GROUP BY p.id),
         lt AS (
           SELECT gr.product_id, COUNT(*)::bigint AS n,
                  AVG((gr.received_on - po.order_date)::float8) AS mean,
                  COALESCE(STDDEV_SAMP((gr.received_on - po.order_date)::float8), 0) AS sd
           FROM goods_receipts gr JOIN purchase_orders po ON po.id = gr.purchase_order_id
           GROUP BY gr.product_id),
         stk AS (
           SELECT sl.product_id, SUM(sl.quantity - sl.reserved - sl.blocked)::bigint AS available
           FROM stock_levels sl JOIN locations l ON l.id = sl.location_id
           WHERE ($1::uuid IS NULL OR l.warehouse_id = $1) GROUP BY sl.product_id),
         oo AS (
           SELECT pol.product_id, SUM(pol.quantity_ordered - pol.quantity_received)::bigint AS on_order
           FROM purchase_order_lines pol JOIN purchase_orders po ON po.id = pol.purchase_order_id
           WHERE po.status IN ('submitted', 'partially_received')
             AND ($1::uuid IS NULL OR po.warehouse_id = $1)
           GROUP BY pol.product_id)
         SELECT p.id AS product_id, p.sku, p.name, p.criticality_class, p.unit_cost::float8 AS unit_cost,
                p.holding_cost_rate::float8 AS holding_cost_rate, p.stockout_cost::float8 AS stockout_cost,
                p.review_period_days, p.lead_time_days, p.reorder_point, p.reorder_qty,
                dem.mu, dem.sd, dem.mu30, lt.n AS lt_n, lt.mean AS lt_mean, lt.sd AS lt_sd,
                COALESCE(stk.available, 0) AS available, COALESCE(oo.on_order, 0) AS on_order
         FROM products p
         JOIN dem ON dem.product_id = p.id
         LEFT JOIN lt ON lt.product_id = p.id
         LEFT JOIN stk ON stk.product_id = p.id
         LEFT JOIN oo ON oo.product_id = p.id
         WHERE dem.mu > 0 OR p.reorder_point > 0
         ORDER BY p.sku",
        [q.warehouse_id.into(), lookback.into(), q.as_of.unwrap_or_else(|| Utc::now().date_naive()).into()],
    ))
    .all(&s.db)
    .await?;

    let items = rows
        .into_iter()
        .map(|r| {
            let (lead, lead_sd, source) = match (r.lt_n, r.lt_mean, r.lt_sd) {
                (Some(n), Some(m), Some(sd)) if n >= MIN_LT_SAMPLES => (m, sd, "observed_receipts"),
                _ => {
                    let l = r.lead_time_days as f64;
                    (l, l * DEFAULT_LT_CV, "product_default")
                }
            };
            let review = r.review_period_days as f64;
            let sl = service_level(&r.criticality_class, r.stockout_cost, r.unit_cost, r.holding_cost_rate, lead + review);
            let z = norm_inv(sl);
            let ss = safety_stock(z, r.mu, r.sd, lead, lead_sd, review);
            let rop = (r.mu * (lead + review) + ss).ceil();
            let up_to = (rop + r.mu * review).ceil();
            let position = r.available + r.on_order;
            let reorder_now = r.mu > 0.0 && (position as f64) <= rop;
            let mut suggested = if reorder_now { (up_to - position as f64).max(0.0).ceil() as i64 } else { 0 };
            if reorder_now && r.reorder_qty > 0 {
                suggested = suggested.max(r.reorder_qty);
            }
            let ratio = (r.mu > 0.0).then(|| r.mu30 / r.mu);
            let trend = match ratio {
                Some(x) if x > 1.25 => "rising",
                Some(x) if x < 0.75 => "falling",
                _ => "stable",
            };
            let action = if r.reorder_point == 0 {
                "set"
            } else {
                let rel = (rop - r.reorder_point as f64) / r.reorder_point as f64;
                if rel > 0.2 { "raise" } else if rel < -0.2 { "lower" } else { "ok" }
            };
            ReplItem {
                product_id: r.product_id,
                sku: r.sku,
                name: r.name,
                criticality_class: r.criticality_class,
                daily_demand_mean: r.mu,
                daily_demand_sd: r.sd,
                recent_demand_ratio: ratio,
                demand_trend: trend,
                lead_time_days: lead,
                lead_time_sd_days: lead_sd,
                lead_time_source: source,
                review_period_days: r.review_period_days,
                service_level: sl,
                safety_stock: ss.ceil() as i64,
                recommended_reorder_point: rop as i64,
                order_up_to_level: up_to as i64,
                current_reorder_point: r.reorder_point,
                reorder_point_action: action,
                available: r.available,
                on_order: r.on_order,
                inventory_position: position,
                coverage_days: (r.mu > 0.0).then(|| r.available as f64 / r.mu),
                reorder_now,
                suggested_order_qty: suggested,
            }
        })
        .collect();
    Ok(Json(items))
}

// ---- ABC / XYZ -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AbcQuery {
    warehouse_id: Option<Uuid>,
    /// default 365, 28..=1095
    lookback_days: Option<i32>,
    as_of: Option<NaiveDate>,
}

#[derive(Debug, FromQueryResult)]
struct AbcRow {
    product_id: Uuid,
    sku: String,
    name: String,
    units: f64,
    value: f64,
    mu: f64,
    sd: f64,
}

#[derive(Serialize)]
pub struct AbcItem {
    product_id: Uuid,
    sku: String,
    name: String,
    units_issued: f64,
    issue_value: f64,
    cumulative_share: f64,
    abc: &'static str,
    /// coefficient of variation of weekly demand
    demand_cv: f64,
    xyz: &'static str,
    policy: &'static str,
}

fn policy(abc: &str, xyz: &str) -> &'static str {
    match (abc, xyz) {
        ("A", "X") | ("A", "Y") => "tight control: frequent review, high service level, supplier collaboration",
        ("A", _) => "high value, erratic: hold safety stock selectively, consider make-to-order or consignment",
        ("B", "X") | ("B", "Y") => "standard statistical replenishment",
        ("B", _) | ("C", "Z") => "review less often; order on demand where lead time allows",
        _ => "low-touch: larger, infrequent orders",
    }
}

/// Value ranking (A = first 80% of issue value, B = next 15%, C = rest) crossed with demand
/// variability (X: CV<0.5, Y: <1.0, Z: >=1.0) on weekly demand.
async fn abc_xyz(State(s): State<AppState>, Query(q): Query<AbcQuery>) -> AppResult<Json<Vec<AbcItem>>> {
    let lookback = q.lookback_days.unwrap_or(365);
    if !(28..=1095).contains(&lookback) {
        return Err(AppError::bad("lookback_days must be between 28 and 1095"));
    }
    let rows = AbcRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH weeks AS (
           SELECT generate_series(date_trunc('week', $3::date - $2::int)::date,
                                  date_trunc('week', $3::date)::date, interval '1 week')::date AS w),
         iss AS (
           SELECT sm.product_id, date_trunc('week', sm.created_at)::date AS w, SUM(-sm.quantity_delta)::float8 AS q
           FROM stock_movements sm JOIN locations l ON l.id = sm.location_id
           WHERE sm.movement_type = 'issue' AND sm.created_at::date >= $3::date - $2::int
             AND ($1::uuid IS NULL OR l.warehouse_id = $1)
           GROUP BY 1, 2)
         SELECT p.id AS product_id, p.sku, p.name,
                SUM(COALESCE(i.q, 0))::float8 AS units,
                (SUM(COALESCE(i.q, 0)) * p.unit_cost::float8)::float8 AS value,
                AVG(COALESCE(i.q, 0))::float8 AS mu,
                COALESCE(STDDEV_SAMP(COALESCE(i.q, 0)), 0)::float8 AS sd
         FROM products p CROSS JOIN weeks LEFT JOIN iss i ON i.product_id = p.id AND i.w = weeks.w
         GROUP BY p.id, p.sku, p.name, p.unit_cost
         HAVING SUM(COALESCE(i.q, 0)) > 0
         ORDER BY value DESC, p.sku",
        [q.warehouse_id.into(), lookback.into(), q.as_of.unwrap_or_else(|| Utc::now().date_naive()).into()],
    ))
    .all(&s.db)
    .await?;

    let total: f64 = rows.iter().map(|r| r.value).sum();
    let mut cum = 0.0;
    let items = rows
        .into_iter()
        .map(|r| {
            let before = if total > 0.0 { cum / total } else { 0.0 };
            cum += r.value;
            let abc = if before < 0.80 { "A" } else if before < 0.95 { "B" } else { "C" };
            let cv = if r.mu > 0.0 { r.sd / r.mu } else { 0.0 };
            let xyz = if cv < 0.5 { "X" } else if cv < 1.0 { "Y" } else { "Z" };
            AbcItem {
                product_id: r.product_id,
                sku: r.sku,
                name: r.name,
                units_issued: r.units,
                issue_value: r.value,
                cumulative_share: if total > 0.0 { cum / total } else { 0.0 },
                abc,
                demand_cv: cv,
                xyz,
                policy: policy(abc, xyz),
            }
        })
        .collect();
    Ok(Json(items))
}

// ---- forecasts -------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ForecastIn {
    warehouse_id: Uuid,
    product_id: Uuid,
    period_start: NaiveDate,
    period_days: Option<i32>,
    /// baseline | adjusted (default baseline)
    forecast_type: Option<String>,
    forecast_qty: f64,
    uplift_pct: Option<f64>,
}

#[derive(Deserialize)]
pub struct ForecastBatch {
    items: Vec<ForecastIn>,
}

/// Upsert up to 5,000 forecasts per call.
async fn upsert_forecasts(
    State(s): State<AppState>,
    Json(b): Json<ForecastBatch>,
) -> AppResult<(StatusCode, Json<Value>)> {
    if b.items.is_empty() || b.items.len() > 5000 {
        return Err(AppError::bad("items must contain 1 to 5000 forecasts"));
    }
    let txn = s.db.begin().await?;
    for f in &b.items {
        let ftype = f.forecast_type.as_deref().unwrap_or("baseline");
        if !matches!(ftype, "baseline" | "adjusted") {
            return Err(AppError::bad("forecast_type must be baseline or adjusted"));
        }
        if f.forecast_qty < 0.0 || f.period_days.unwrap_or(7) <= 0 {
            return Err(AppError::bad("forecast_qty must be >= 0 and period_days > 0"));
        }
        txn.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO demand_forecasts (id, warehouse_id, product_id, period_start, period_days, \
                 forecast_type, forecast_qty, uplift_pct) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
             ON CONFLICT (warehouse_id, product_id, period_start, forecast_type) \
             DO UPDATE SET forecast_qty = EXCLUDED.forecast_qty, period_days = EXCLUDED.period_days, \
                           uplift_pct = EXCLUDED.uplift_pct",
            [
                Uuid::new_v4().into(),
                f.warehouse_id.into(),
                f.product_id.into(),
                f.period_start.into(),
                f.period_days.unwrap_or(7).into(),
                ftype.into(),
                f.forecast_qty.into(),
                f.uplift_pct.into(),
            ],
        ))
        .await?;
    }
    txn.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({ "upserted": b.items.len(), "at": Utc::now() }))))
}

#[derive(Deserialize)]
pub struct AccQuery {
    warehouse_id: Option<Uuid>,
    product_id: Option<Uuid>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Debug, FromQueryResult)]
struct AccRow {
    forecast_type: String,
    n: i64,
    forecast: f64,
    actual: f64,
    abs_err: f64,
    err: f64,
}

#[derive(Serialize)]
pub struct AccItem {
    forecast_type: String,
    periods: i64,
    forecast_units: f64,
    actual_units: f64,
    /// sum|forecast-actual| / sum actual
    wape: Option<f64>,
    /// sum(forecast-actual) / sum actual; positive = over-forecasting
    bias: Option<f64>,
    mean_abs_error: f64,
}

/// Compares each forecast with the units actually issued in its period (completed periods only),
/// split by forecast type so manual "adjusted" forecasts can be checked against baselines.
async fn forecast_accuracy(
    State(s): State<AppState>,
    Query(q): Query<AccQuery>,
) -> AppResult<Json<Vec<AccItem>>> {
    let rows = AccRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH f AS (
           SELECT * FROM demand_forecasts
           WHERE period_start + period_days <= CURRENT_DATE
             AND ($1::uuid IS NULL OR warehouse_id = $1) AND ($2::uuid IS NULL OR product_id = $2)
             AND ($3::date IS NULL OR period_start >= $3) AND ($4::date IS NULL OR period_start <= $4)),
         act AS (
           SELECT f.id, COALESCE(SUM(-sm.quantity_delta), 0)::float8 AS actual
           FROM f
           LEFT JOIN locations l ON l.warehouse_id = f.warehouse_id
           LEFT JOIN stock_movements sm ON sm.location_id = l.id AND sm.product_id = f.product_id
                AND sm.movement_type = 'issue'
                AND sm.created_at::date >= f.period_start AND sm.created_at::date < f.period_start + f.period_days
           GROUP BY f.id)
         SELECT f.forecast_type, COUNT(*)::bigint AS n, SUM(f.forecast_qty)::float8 AS forecast,
                SUM(a.actual)::float8 AS actual, SUM(ABS(f.forecast_qty - a.actual))::float8 AS abs_err,
                SUM(f.forecast_qty - a.actual)::float8 AS err
         FROM f JOIN act a ON a.id = f.id GROUP BY f.forecast_type ORDER BY f.forecast_type",
        [q.warehouse_id.into(), q.product_id.into(), q.from.into(), q.to.into()],
    ))
    .all(&s.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                AccItem {
                    forecast_type: r.forecast_type,
                    periods: r.n,
                    forecast_units: r.forecast,
                    actual_units: r.actual,
                    wape: (r.actual > 0.0).then(|| r.abs_err / r.actual),
                    bias: (r.actual > 0.0).then(|| r.err / r.actual),
                    mean_abs_error: r.abs_err / r.n as f64,
                }
            })
            .collect(),
    ))
}
