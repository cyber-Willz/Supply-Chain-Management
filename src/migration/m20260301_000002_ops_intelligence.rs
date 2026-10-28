use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(include_str!("v2.sql")).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS supplier_invoice_lines, supplier_invoices, demand_forecasts, \
                 quality_incidents, goods_receipts CASCADE; \
                 ALTER TABLE purchase_order_lines DROP COLUMN promised_date, DROP COLUMN budget_unit_cost; \
                 ALTER TABLE purchase_orders DROP COLUMN order_date, DROP COLUMN po_type, DROP COLUMN currency, \
                   DROP COLUMN payment_terms_days, DROP COLUMN contract_id; \
                 DROP TABLE IF EXISTS contracts CASCADE; \
                 ALTER TABLE stock_levels DROP CONSTRAINT stock_levels_reserved_blocked_check, \
                   DROP COLUMN blocked, DROP COLUMN expiry_date; \
                 ALTER TABLE locations DROP COLUMN storage_class; \
                 ALTER TABLE suppliers DROP COLUMN country, DROP COLUMN region, DROP COLUMN tier, DROP COLUMN esg_score, \
                   DROP COLUMN financial_stability_score, DROP COLUMN geopolitical_risk_index, \
                   DROP COLUMN alternatives_available, DROP COLUMN environmental_compliance, \
                   DROP COLUMN previous_disruptions, DROP COLUMN payment_terms_days; \
                 ALTER TABLE products DROP COLUMN criticality_class, DROP COLUMN storage_class, DROP COLUMN shelf_life_days, \
                   DROP COLUMN is_repairable, DROP COLUMN review_period_days, DROP COLUMN lead_time_days, \
                   DROP COLUMN holding_cost_rate, DROP COLUMN stockout_cost;",
            )
            .await?;
        Ok(())
    }
}
