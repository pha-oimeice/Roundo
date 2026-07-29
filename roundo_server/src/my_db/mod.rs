//! This module only provides interface. \
//! Read and Write are predefined.

pub mod game_session;
mod pg_pool;
mod sql_rows;
pub mod user;

#[allow(unused_imports)]
pub use self::pg_pool::{PG_POOL, get_pg_pool};

pub async fn initialize_database() -> anyhow::Result<()> {
    pg_pool::initialize_pg_pool().await?;
    let pool = get_pg_pool();
    game_session::ensure_schema(pool).await?;
    game_session::ensure_public_user_and_session().await?;
    Ok(())
}
