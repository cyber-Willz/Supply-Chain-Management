use sea_orm_migration::prelude::*;

mod m20260101_000001_init;
mod m20260301_000002_ops_intelligence;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260101_000001_init::Migration),
            Box::new(m20260301_000002_ops_intelligence::Migration),
        ]
    }
}
