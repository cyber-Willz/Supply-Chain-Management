# supply_chain (v0.2)

Supply chain, logistics, warehouse and inventory management API.
Rust · Axum 0.7 · SeaORM 1.1 · PostgreSQL.

## Quick start

```bash
docker compose up -d              # PostgreSQL on :5432
cp .env.example .env              # set API_KEY (or remove it to disable auth)
cargo run                         # runs migrations, serves on BIND_ADDR (default 0.0.0.0:8080)
API_KEY=change-me python3 scripts/smoke_test.py      # core flows
API_KEY=change-me python3 scripts/smoke_test_v2.py   # cold chain, FEFO, quarantine, contracts, invoices, analytics
cargo test                        # unit tests
```

All endpoints live under `/api/v1` and require `x-api-key` when `API_KEY` is set. `GET /health` is open.
Lists accept `limit` (default 50, max 500) and `offset`.

## Domain model

| Area | Tables |
|---|---|
| Master data | `warehouses`, `locations` (bins), `products`, `suppliers`, `customers`, `carriers` |
| Inventory | `stock_levels` (product × bin × lot, with `quantity` and `reserved`), `stock_movements` (append-only ledger) |
| Procurement | `purchase_orders`, `purchase_order_lines` |
| Fulfilment | `sales_orders`, `sales_order_lines`, `allocations`, `shipments`, `shipment_events` |

Integrity rules are enforced by the database as well as the API: `quantity >= 0`, `reserved <= quantity`,
`received <= ordered`, status CHECK constraints. Stock changes are single atomic SQL statements inside
transactions, and allocation locks candidate rows with `FOR UPDATE`, so concurrent requests cannot oversell.

## Flows

**Inbound:** `POST /purchase-orders` (draft) → `/submit` → `/receive` (per line, into a bin of the PO's
warehouse, optional `lot_number`; partial receipts supported) → status `partially_received` / `received`.
`/cancel` is allowed until something is received.

**Outbound:** `POST /sales-orders` (draft) → `/confirm` reserves stock oldest-first from the order's
warehouse → status `confirmed`, `partially_allocated` or `backordered`. `/allocate` retries after restock.
`/ship` consumes the reserved stock, writes `issue` ledger entries and creates a shipment (`in_transit`).
`/cancel` releases reservations (not allowed once anything shipped).

**Tracking:** `POST /shipments/:id/events` (`in_transit`, `out_for_delivery`, `delivered`, `exception`),
`GET /tracking/:tracking_number`.

**Inventory:** `GET /inventory/stock`, `GET /inventory/movements`, `POST /inventory/adjust`
(+/- with reason), `POST /inventory/transfer` (bin to bin, or across warehouses).

**Reports:** `GET /reports/low-stock` (available + open PO quantity vs reorder point, with suggested order
quantity), `GET /reports/valuation` (units and value at unit cost per warehouse).

## Endpoint reference

```
POST/GET   /warehouses            GET /warehouses/:id
POST/GET   /warehouses/:id/locations
POST/GET   /products              GET/PATCH /products/:id      (?q= matches SKU or name)
POST/GET   /suppliers   /customers   /carriers
POST/GET   /purchase-orders       GET /purchase-orders/:id
POST       /purchase-orders/:id/{submit,cancel,receive}
POST/GET   /sales-orders          GET /sales-orders/:id
POST       /sales-orders/:id/{confirm,allocate,ship,cancel}
GET        /shipments  /shipments/:id        POST /shipments/:id/events
GET        /tracking/:tracking_number
GET        /inventory/stock  /inventory/movements
POST       /inventory/adjust  /inventory/transfer
GET        /reports/low-stock  /reports/valuation
```

## Example

```bash
H='content-type: application/json'; K='x-api-key: change-me'; B=http://localhost:8080/api/v1
curl -s -H "$H" -H "$K" -d '{"code":"MAIN","name":"Main DC"}' $B/warehouses
curl -s -H "$H" -H "$K" -d '{"sku":"W-1","name":"Widget","unit_cost":"2.50","list_price":"5","reorder_point":20,"reorder_qty":100}' $B/products
```

## v0.2: dataset-driven additions (see ANALYSIS.md for the evidence)

```
POST/GET   /contracts                         orders outside the contract window are rejected
POST       /purchase-orders/:id/close         close a partially received order as short-shipped
POST       /purchase-orders/:id/invoices      automatic 3-way match -> matched | disputed
GET        /invoices  /invoices/:id           POST /invoices/:id/{approve,pay}
POST/GET   /quality-incidents                 optional quarantine of stock (blocked quantity)
POST       /quality-incidents/:id/resolve     release and/or scrap quarantined units
GET/PATCH  /suppliers/:id                     risk inputs, tier, ESG, payment terms
GET        /suppliers/:id/scorecard           observed on-time / in-full / quality + structural risk
GET        /reports/supplier-scorecards       all suppliers (?since=YYYY-MM-DD)
GET        /reports/replenishment             statistical safety stock and reorder points
                                              (?warehouse_id, lookback_days, as_of)
GET        /reports/abc-xyz                   value x variability classes with policy hints
POST       /forecasts                         bulk upsert (<= 5000) baseline/adjusted forecasts
GET        /reports/forecast-accuracy         WAPE and bias per forecast type
GET        /reports/procurement-controls      no-contract spend, budget variance, dispute rate
```

* Products gain `criticality_class` (A/B/C), `storage_class`, `shelf_life_days`, `review_period_days`,
  `lead_time_days`, `holding_cost_rate`, `stockout_cost`. Bins gain `storage_class`; put-away into a bin of a
  different class is rejected. Stock lots carry `expiry_date` (defaulting from shelf life) and allocation is
  first-expiry-first-out, skipping expired and quarantined units. Availability = on-hand - reserved - blocked.
* Receipts accept `received_on`, `lot_number`, `expiry_date`; orders accept `order_date`, `promised_date`,
  `po_type`, `currency`, `payment_terms_days`, `contract_id`, `budget_unit_cost`.
* `scripts/import_aerospace.py` loads the aerospace dataset (600k+ rows) for benchmarking; the reports
  reproduce independent pandas calculations exactly.

## Notes

- Quantities are whole units (`BIGINT`); money is `NUMERIC(14,4)` (JSON strings).
- Costing is a single current `unit_cost` per product (no FIFO/average layers); currencies are labelled, not converted.
- Sales orders ship everything currently reserved in one shipment.
- Status values are TEXT columns with CHECK constraints rather than Postgres enums, so adding a status is a simple migration.
