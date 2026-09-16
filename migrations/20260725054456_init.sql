-- Add migration script here
CREATE TABLE roundo_users (
    id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    permission INT NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'active'
);

CREATE TABLE roundo_game_sessions (
    id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    owner_user_id INT NOT NULL REFERENCES roundo_users(id) ON DELETE CASCADE,
    name TEXT NOT NULL DEFAULT gen_random_uuid(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMPTZ,
    closed_at TIMESTAMPTZ,
    UNIQUE(owner_user_id, name)
);

CREATE VIEW roundo_session_view AS (
SELECT
    ru.id AS owner_uid,
    ru.name AS owner_name,
    rgs.id AS session_id,
    rgs.name AS session_name
FROM roundo_users AS ru
    INNER JOIN roundo_game_sessions AS rgs ON ru.id = rgs.owner_user_id
ORDER BY owner_uid, session_id
);

DELETE FROM roundo_game_sessions
WHERE owner_user_id IN (
    SELECT id
    FROM roundo_users
    WHERE roundo_users.name IN ('root', 'public')
);

DELETE FROM roundo_users
WHERE roundo_users.name IN ('root', 'public');

INSERT INTO roundo_users (name)
SELECT 'root';

INSERT INTO roundo_users (name)
SELECT 'public';

INSERT INTO roundo_game_sessions (owner_user_id, name)
SELECT
    id,
    'roundo_test_session'
FROM roundo_users
WHERE roundo_users.name = 'public';
