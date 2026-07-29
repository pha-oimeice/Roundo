use sqlx::PgPool;
use std::sync::OnceLock;

pub static PG_POOL: OnceLock<PgPool> = OnceLock::new();

pub async fn initialize_pg_pool() -> anyhow::Result<()> {
    if PG_POOL.get().is_some() {
        return Ok(());
    }
    let database_url = crate::config::SERVER_CONFIG.database.url.clone();
    let pool_size = crate::config::SERVER_CONFIG.database.pool_size;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(pool_size)
        .connect(database_url.as_str())
        .await?;
    let _ = PG_POOL.set(pool);
    log::info!("Database connected.");
    Ok(())
}

pub fn get_pg_pool() -> &'static PgPool {
    PG_POOL
        .get()
        .expect("database pool must be initialized before use")
}
