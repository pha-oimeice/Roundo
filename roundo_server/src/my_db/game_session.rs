//! Schema bootstrap and lookup for the public game session.

use anyhow::Context;
use sqlx::PgPool;

pub const PUBLIC_USER_NAME: &str = "public";
pub const PUBLIC_SESSION_NAME: &str = "default";

#[derive(Debug, Clone, sqlx::FromRow)]
/// Database identity and ownership of one game session.
pub struct GameSession {
    pub id: i32,
    pub owner_user_id: i32,
    pub name: Option<String>,
}

/// Applies idempotent schema statements in dependency order.
pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    apply_schema_statement(
        pool,
        "drop_legacy_user_view",
        "DROP VIEW IF EXISTS roundo_user_view",
    )
    .await?;
    apply_schema_statement(
        pool,
        "create_users_table",
        r#"
        CREATE TABLE IF NOT EXISTS roundo_users (
            id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .await?;

    // Remove the obsolete provider-account mapping. Authentication providers
    // belong behind an external adapter, not in the game database schema.
    apply_schema_statement(
        pool,
        "drop_legacy_authentications_table",
        "DROP TABLE IF EXISTS roundo_user_authentications",
    )
    .await?;
    apply_schema_statement(
        pool,
        "create_game_sessions_table",
        r#"
        CREATE TABLE IF NOT EXISTS roundo_game_sessions (
            id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            owner_user_id INT NOT NULL REFERENCES roundo_users(id) ON DELETE CASCADE,
            name TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
            expires_at TIMESTAMPTZ,
            closed_at TIMESTAMPTZ
        )
        "#,
    )
    .await?;
    apply_schema_statement(
        pool,
        "add_game_session_name",
        "ALTER TABLE roundo_game_sessions ADD COLUMN IF NOT EXISTS name TEXT",
    )
    .await?;
    apply_schema_statement(
        pool,
        "add_game_session_created_at",
        "ALTER TABLE roundo_game_sessions ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP",
    )
    .await?;
    apply_schema_statement(
        pool,
        "add_game_session_expires_at",
        "ALTER TABLE roundo_game_sessions ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ",
    )
    .await?;
    apply_schema_statement(
        pool,
        "relax_game_session_expiry",
        "ALTER TABLE roundo_game_sessions ALTER COLUMN expires_at DROP NOT NULL",
    )
    .await?;
    apply_schema_statement(
        pool,
        "add_game_session_closed_at",
        "ALTER TABLE roundo_game_sessions ADD COLUMN IF NOT EXISTS closed_at TIMESTAMPTZ",
    )
    .await?;
    apply_schema_statement(
        pool,
        "drop_legacy_session_token",
        "ALTER TABLE roundo_game_sessions DROP COLUMN IF EXISTS session_token CASCADE",
    )
    .await?;
    apply_schema_statement(
        pool,
        "create_session_owner_name_index",
        r#"
        CREATE UNIQUE INDEX IF NOT EXISTS roundo_game_sessions_owner_name_unique
            ON roundo_game_sessions(owner_user_id, name)
        "#,
    )
    .await?;
    apply_schema_statement(
        pool,
        "create_user_view",
        r#"
        CREATE OR REPLACE VIEW roundo_user_view AS
        SELECT
            ru.id AS owner_uid,
            ru.name AS owner_name,
            rgs.id AS session_id,
            rgs.name AS session_name
        FROM roundo_users AS ru
            LEFT JOIN roundo_game_sessions AS rgs ON ru.id = rgs.owner_user_id
        ORDER BY owner_uid, session_id
        "#,
    )
    .await?;

    log::info!("database schema is ready");
    Ok(())
}

// Adds operation context to every schema statement failure.
async fn apply_schema_statement(
    pool: &PgPool,
    operation: &'static str,
    statement: &'static str,
) -> anyhow::Result<()> {
    let query = sqlx::query(statement);
    let execution = query.execute(pool);
    let result = execution
        .await
        .with_context(|| format!("database schema operation `{operation}` failed"))?;
    log::debug!(
        "database schema operation completed: operation={operation}, rows_affected={}",
        result.rows_affected()
    );
    Ok(())
}

/// Creates or reopens the built-in public user and default session.
pub async fn ensure_public_user_and_session() -> anyhow::Result<GameSession> {
    let pool = crate::my_db::get_pg_pool();
    let user_id = sqlx::query_scalar::<_, i32>(
        r#"
        INSERT INTO roundo_users (name)
        VALUES ($1)
        ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(PUBLIC_USER_NAME)
    .fetch_one(pool)
    .await?;

    let session = sqlx::query_as::<_, GameSession>(
        r#"
        INSERT INTO roundo_game_sessions (owner_user_id, name)
        VALUES ($1, $2)
        ON CONFLICT (owner_user_id, name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id, owner_user_id, name
        "#,
    )
    .bind(user_id)
    .bind(PUBLIC_SESSION_NAME)
    .fetch_one(pool)
    .await?;

    log::info!(
        "Public user/session ready: user={}, session={}",
        session.owner_user_id,
        session.id
    );

    Ok(session)
}

/// Fetches the built-in public session.
pub async fn public_session() -> anyhow::Result<GameSession> {
    ensure_public_user_and_session().await
}

/// Reports whether a session exists and currently accepts joins.
pub async fn is_session_open(session_id: i32) -> anyhow::Result<bool> {
    let pool = crate::my_db::get_pg_pool();
    let exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM roundo_game_sessions
            WHERE id = $1
              AND closed_at IS NULL
              AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
        )
        "#,
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;

    Ok(exists)
}
