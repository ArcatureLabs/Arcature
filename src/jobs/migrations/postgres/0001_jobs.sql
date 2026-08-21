-- 0001_jobs.sql (PostgreSQL) -- the durable job queue.
--
-- One table, arcature_jobs, with a status lifecycle:
--   pending -> running -> {succeeded | pending(retry) | dead | cancelled}
--
-- Statements are separated by a line reading `--;;` and executed one at a
-- time, so the file never relies on multi-statement support.
--
-- `id` has no default: the enqueue path generates the UUID client-side, which
-- keeps the insert free of RETURNING (MySQL has none) and of pgcrypto.
-- `updated_at` is set by every mutating statement rather than by a trigger,
-- because two of the three dialects would need a different trigger dialect to
-- say the same thing.

CREATE TABLE IF NOT EXISTS arcature_jobs (
    id              UUID PRIMARY KEY,
    kind            TEXT        NOT NULL,
    version         SMALLINT    NOT NULL,
    payload         JSONB       NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'pending',
    attempts        INTEGER     NOT NULL DEFAULT 0,
    max_attempts    INTEGER     NOT NULL,
    run_at          TIMESTAMPTZ NOT NULL,
    available_at    TIMESTAMPTZ NOT NULL,
    locked_at       TIMESTAMPTZ,
    locked_by       TEXT,
    lease_seconds   INTEGER     NOT NULL DEFAULT 300,
    last_error      TEXT,
    last_error_kind TEXT,
    failed_at       TIMESTAMPTZ,
    claim_token     UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

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
