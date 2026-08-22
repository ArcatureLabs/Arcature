-- 0001_password_resets.sql (MySQL 8) -- one-time password-reset tokens.
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
-- `TEXT` column cannot be indexed without naming a prefix length. 191 is above
-- the longest address RFC 5321 permits (254 octets is the path, 191 characters
-- the local part plus domain in practice) for every address this store will
-- ever see; a longer subject is refused by the insert rather than silently
-- truncated, because `sql_mode` in MySQL 8 defaults to `STRICT_TRANS_TABLES`.
--
-- `DATETIME(6)` holds UTC with microsecond resolution and, unlike
-- `TIMESTAMP`, is not silently converted between session time zones. Every
-- comparison against it uses `UTC_TIMESTAMP(6)` for the same reason.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_password_resets (
    id            BINARY(16)   NOT NULL PRIMARY KEY,
    secret_digest BINARY(32)   NOT NULL,
    subject       VARCHAR(191) NOT NULL,
    expires_at    DATETIME(6)  NOT NULL,
    created_at    DATETIME(6)  NOT NULL,
    KEY arcature_password_resets_subject_idx (subject),
    KEY arcature_password_resets_expires_at_idx (expires_at)
) ENGINE=InnoDB
