use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("init.sql"))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS shipment_events, allocations, shipments, sales_order_lines, \
                 sales_orders, purchase_order_lines, purchase_orders, stock_movements, stock_levels, \
                 carriers, customers, suppliers, products, locations, warehouses CASCADE",
            )
            .await?;
        Ok(())
    }
}
