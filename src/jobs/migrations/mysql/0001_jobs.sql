-- 0001_jobs.sql (MySQL 8) -- the durable job queue.
--
-- Same table and same lifecycle as the other dialects. MySQL specifics:
--
--   * UUIDs are BINARY(16); SQLx encodes `Uuid` as 16 raw bytes.
--   * Timestamps are DATETIME(6) holding UTC. The queue writes and compares
--     them with UTC_TIMESTAMP(6), never NOW(), so the session time zone
--     cannot shift the lease arithmetic.
--   * Indexes are declared inside CREATE TABLE: MySQL has no
--     `CREATE INDEX IF NOT EXISTS`, so a separate statement would fail on the
--     second run. They are not partial for the same reason -- MySQL has no
--     partial indexes -- so `status` leads the key instead.
--   * CHECK constraints are enforced from MySQL 8.0.16; on an older server
--     they parse and are ignored, which is why the Rust side never relies on
--     them for correctness.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_jobs (
    id              BINARY(16)   NOT NULL PRIMARY KEY,
    kind            VARCHAR(191) NOT NULL,
    version         SMALLINT     NOT NULL,
    payload         JSON         NOT NULL,
    status          VARCHAR(16)  NOT NULL DEFAULT 'pending',
    attempts        INT          NOT NULL DEFAULT 0,
    max_attempts    INT          NOT NULL,
    run_at          DATETIME(6)  NOT NULL,
    available_at    DATETIME(6)  NOT NULL,
    locked_at       DATETIME(6)  NULL,
    locked_by       VARCHAR(128) NULL,
    lease_seconds   INT          NOT NULL DEFAULT 300,
    last_error      TEXT         NULL,
    last_error_kind VARCHAR(32)  NULL,
    failed_at       DATETIME(6)  NULL,
    claim_token     BINARY(16)   NULL,
    created_at      DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at      DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    KEY arcature_jobs_claim_idx (status, available_at, id),
    KEY arcature_jobs_sweep_idx (status, locked_at),
    KEY arcature_jobs_kind_idx (kind, status),

    CONSTRAINT arcature_jobs_status_check
        CHECK (status IN ('pending', 'running', 'succeeded', 'dead', 'cancelled')),
    CONSTRAINT arcature_jobs_attempts_check      CHECK (attempts >= 0),
    CONSTRAINT arcature_jobs_max_attempts_check  CHECK (max_attempts >= 1),
    CONSTRAINT arcature_jobs_version_check       CHECK (version >= 1),
    CONSTRAINT arcature_jobs_lease_seconds_check CHECK (lease_seconds >= 1)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4
