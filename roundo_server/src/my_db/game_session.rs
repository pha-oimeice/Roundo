use sqlx::PgPool;

pub const PUBLIC_USER_NAME: &str = "public";
pub const PUBLIC_SESSION_NAME: &str = "default";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GameSession {
    pub id: i32,
    pub owner_user_id: i32,
    pub name: Option<String>,
}

pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query("DROP VIEW IF EXISTS roundo_user_view")
        .execute(pool)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS roundo_users (
            id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS roundo_user_authentications (
            id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            user_id INT NOT NULL REFERENCES roundo_users(id) ON DELETE CASCADE,
            provider TEXT NOT NULL,
            provider_subject TEXT NOT NULL,
            UNIQUE(provider, provider_subject)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
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
    .execute(pool)
    .await?;

    sqlx::query("ALTER TABLE roundo_game_sessions ADD COLUMN IF NOT EXISTS name TEXT")
        .execute(pool)
        .await?;
    sqlx::query(
        "ALTER TABLE roundo_game_sessions ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP",
    )
    .execute(pool)
    .await?;
    sqlx::query("ALTER TABLE roundo_game_sessions ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ")
        .execute(pool)
        .await?;
    sqlx::query("ALTER TABLE roundo_game_sessions ALTER COLUMN expires_at DROP NOT NULL")
        .execute(pool)
        .await?;
    sqlx::query("ALTER TABLE roundo_game_sessions ADD COLUMN IF NOT EXISTS closed_at TIMESTAMPTZ")
        .execute(pool)
        .await?;
    sqlx::query("ALTER TABLE roundo_game_sessions DROP COLUMN IF EXISTS session_token CASCADE")
        .execute(pool)
        .await?;

    sqlx::query(
        r#"
        CREATE UNIQUE INDEX IF NOT EXISTS roundo_game_sessions_owner_name_unique
            ON roundo_game_sessions(owner_user_id, name)
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
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
    .execute(pool)
    .await?;

    Ok(())
}

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

pub async fn public_session() -> anyhow::Result<GameSession> {
    ensure_public_user_and_session().await
}

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
