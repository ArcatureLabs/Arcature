-- 0001_jobs.sql (SQLite) -- the durable job queue.
--
-- Same table and same lifecycle as the other dialects, with two storage
-- differences SQLite forces:
--
--   * UUIDs are BLOBs. SQLx encodes `Uuid` as 16 raw bytes here, so a text
--     column would silently compare wrong.
--   * Timestamps are INTEGER epoch milliseconds. SQLite has no timestamp
--     type; text timestamps only compare correctly while every writer agrees
--     on the exact format, and integers always do.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_jobs (
    id              BLOB    PRIMARY KEY NOT NULL,
    kind            TEXT    NOT NULL,
    version         INTEGER NOT NULL,
    payload         TEXT    NOT NULL,
    status          TEXT    NOT NULL DEFAULT 'pending',
    attempts        INTEGER NOT NULL DEFAULT 0,
    max_attempts    INTEGER NOT NULL,
    run_at          INTEGER NOT NULL,
    available_at    INTEGER NOT NULL,
    locked_at       INTEGER,
    locked_by       TEXT,
    lease_seconds   INTEGER NOT NULL DEFAULT 300,
    last_error      TEXT,
    last_error_kind TEXT,
    failed_at       INTEGER,
    claim_token     BLOB,
    created_at      INTEGER NOT NULL
        DEFAULT (CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)),
    updated_at      INTEGER NOT NULL
        DEFAULT (CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)),

    CONSTRAINT arcature_jobs_status_check
        CHECK (status IN ('pending', 'running', 'succeeded', 'dead', 'cancelled')),
    CONSTRAINT arcature_jobs_attempts_check      CHECK (attempts >= 0),
    CONSTRAINT arcature_jobs_max_attempts_check  CHECK (max_attempts >= 1),
    CONSTRAINT arcature_jobs_version_check       CHECK (version >= 1),
    CONSTRAINT arcature_jobs_lease_seconds_check CHECK (lease_seconds >= 1)
)
--;;
-- The claim path: pending rows ordered by availability.
CREATE INDEX IF NOT EXISTS arcature_jobs_claim_idx
    ON arcature_jobs (available_at, id)
    WHERE status = 'pending'
--;;
-- The sweep path: running rows by lock time.
CREATE INDEX IF NOT EXISTS arcature_jobs_sweep_idx
    ON arcature_jobs (locked_at)
    WHERE status = 'running'
--;;
-- Inspection and admin.
CREATE INDEX IF NOT EXISTS arcature_jobs_kind_idx
    ON arcature_jobs (kind, status)
