-- 0001_jobs.sql — the durable PostgreSQL job queue.
--
-- One table, arcature_jobs, with a status lifecycle:
--   pending -> running -> {succeeded | pending(retry) | dead | cancelled}
--
-- The claim is fenced by claim_token (added in 0002_claim_token.sql).
-- Partial indexes keep the hot paths (claim, sweep) cheap.

CREATE TABLE IF NOT EXISTS arcature_jobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind            TEXT        NOT NULL,
    version         SMALLINT    NOT NULL,
    payload         JSONB       NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'pending',
    attempts        INTEGER     NOT NULL DEFAULT 0,
    max_attempts    INTEGER     NOT NULL,
    run_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    available_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_at       TIMESTAMPTZ,
    locked_by       TEXT,
    lease_seconds   INTEGER     NOT NULL DEFAULT 300,
    last_error      TEXT,
    last_error_kind TEXT,
    failed_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT arcature_jobs_status_check
        CHECK (status IN ('pending', 'running', 'succeeded', 'dead', 'cancelled')),
    CONSTRAINT arcature_jobs_attempts_check
        CHECK (attempts >= 0),
    CONSTRAINT arcature_jobs_max_attempts_check
        CHECK (max_attempts >= 1),
    CONSTRAINT arcature_jobs_version_check
        CHECK (version >= 1),
    CONSTRAINT arcature_jobs_lease_seconds_check
        CHECK (lease_seconds >= 1)
);

-- Bump updated_at on every UPDATE.
CREATE OR REPLACE FUNCTION arcature_jobs_touch_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS arcature_jobs_updated_at ON arcature_jobs;
CREATE TRIGGER arcature_jobs_updated_at
    BEFORE UPDATE ON arcature_jobs
    FOR EACH ROW
    EXECUTE FUNCTION arcature_jobs_touch_updated_at();

-- Partial index for the claim path: pending jobs ordered by availability.
CREATE INDEX IF NOT EXISTS arcature_jobs_claim_idx
    ON arcature_jobs (available_at, id)
    WHERE status = 'pending';

-- Partial index for the sweep path: running jobs by locked_at.
CREATE INDEX IF NOT EXISTS arcature_jobs_sweep_idx
    ON arcature_jobs (locked_at)
    WHERE status = 'running';

-- Index for kind/status queries (inspection, admin).
CREATE INDEX IF NOT EXISTS arcature_jobs_kind_idx
    ON arcature_jobs (kind, status);
