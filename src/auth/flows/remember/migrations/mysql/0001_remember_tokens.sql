-- 0001_remember_tokens.sql (MySQL 8) -- rotating remember-me tokens.
--
-- The PostgreSQL file carries the full commentary; this one records only what
-- differs.
--
-- Both indexes are declared inside `CREATE TABLE`. MySQL has no
-- `CREATE INDEX IF NOT EXISTS`, so a separate statement would fail the second
-- time the migration ran -- and the whole point of `IF NOT EXISTS` here is
-- that re-running is harmless.
--
-- `subject` is `VARCHAR(191)` and not `TEXT`, because it is indexed: under
-- `utf8mb4` the legacy 767-byte index prefix allows 191 characters, and a
-- `TEXT` column cannot be indexed without naming a prefix length. A longer
-- subject is refused by the insert rather than silently truncated, because
-- `sql_mode` in MySQL 8 defaults to `STRICT_TRANS_TABLES`.
--
-- `DATETIME(6)` holds UTC with microsecond resolution and, unlike
-- `TIMESTAMP`, is not silently converted between session time zones. Every
-- comparison against it uses `UTC_TIMESTAMP(6)` for the same reason. It also
-- has no epoch limit worth worrying about, which `TIMESTAMP` does -- and this
-- table stores deadlines weeks or months out by design.
--
-- `previous_digest` and `rotated_at` are the only nullable columns, and they
-- are nullable together: NULL in both is how a token that has never rotated is
-- spelled.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_remember_tokens (
    series          BINARY(16)   NOT NULL PRIMARY KEY,
    secret_digest   BINARY(32)   NOT NULL,
    previous_digest BINARY(32),
    rotated_at      DATETIME(6),
    subject         VARCHAR(191) NOT NULL,
    expires_at      DATETIME(6)  NOT NULL,
    created_at      DATETIME(6)  NOT NULL,
    KEY arcature_remember_tokens_subject_idx (subject),
    KEY arcature_remember_tokens_expires_at_idx (expires_at)
) ENGINE=InnoDB
