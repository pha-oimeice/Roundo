use sqlx::PgPool;
use std::sync::OnceLock;

/// Process-wide pool initialized once during server startup.
pub static PG_POOL: OnceLock<PgPool> = OnceLock::new();

/// Creates the shared pool, tolerating concurrent redundant initialization.
pub async fn initialize_pg_pool() -> anyhow::Result<()> {
    let initialized = PG_POOL.get().is_some();
    if initialized {
        return Ok(());
    }
    let database_url = crate::config::SERVER_CONFIG.database.url.clone();
    let pool_size = crate::config::SERVER_CONFIG.database.pool_size;
    let options = sqlx::postgres::PgPoolOptions::new().max_connections(pool_size);
    let connection = options.connect(database_url.as_str());
    let pool = connection
        .await
        .map_err(|error| anyhow::anyhow!("cannot initialize database connection pool: {error}"))?;
    match PG_POOL.set(pool) {
        Ok(()) => log::info!("database connection pool initialized: max_connections={pool_size}"),
        Err(_) => log::debug!(
            "discarded redundant database pool initialized concurrently: max_connections={pool_size}"
        ),
    }
    Ok(())
}

/// Returns the initialized pool; callers must run startup initialization first.
pub fn get_pg_pool() -> &'static PgPool {
    let pool = PG_POOL.get();
    pool.expect("database pool must be initialized before use")
}
