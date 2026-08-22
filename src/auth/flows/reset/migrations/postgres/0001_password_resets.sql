-- 0001_password_resets.sql (PostgreSQL) -- one-time password-reset tokens.
--
-- One table, arcature_password_resets, holding one row per outstanding reset
-- link. A row is deleted the moment the link is redeemed; there is no "spent"
-- flag, because a spent row is a row some future query can forget to filter.
--
-- Statements are separated by a line reading `--;;` and executed one at a
-- time, so the file never relies on multi-statement support.
--
-- `id` is the *public* half of the token: 16 random bytes that travel in the
-- link and are what the lookup indexes on. It is not a secret and is not
-- treated as one.
--
-- `secret_digest` is the SHA-256 of the *secret* half, and the secret half is
-- never stored anywhere. A backup, a replica, or a `SELECT` by a read-only
-- reporting account yields 32 bytes that cannot be turned back into a link.
-- This is the same reasoning as `src/tokens/`, and it matters more here: an
-- API token grants what it was scoped to, while a reset token grants the
-- password itself.
--
-- The digest is SHA-256 and deliberately not argon2, for the reason given at
-- the hashing site in `src/tokens/store.rs` -- the secret is 256 bits of
-- uniform randomness, so a slow hash defends against nothing it was designed
-- to defend against.
--
-- `expires_at` is NOT NULL and there is no "never expires". Every read carries
-- `expires_at > now()`, so a link stops working the moment it lapses whether
-- or not the sweep has run. A reset link that outlives the mail it arrived in
-- is a standing password change sitting in an inbox.

CREATE TABLE IF NOT EXISTS arcature_password_resets (
    id            BYTEA       PRIMARY KEY,
    secret_digest BYTEA       NOT NULL,
    subject       TEXT        NOT NULL,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
)
--;;
-- Issuing clears the subject's outstanding links, and a completed reset
-- clears them again. Both are `WHERE subject = $1`.
CREATE INDEX IF NOT EXISTS arcature_password_resets_subject_idx
    ON arcature_password_resets (subject)
--;;
-- The sweep path: lapsed rows.
CREATE INDEX IF NOT EXISTS arcature_password_resets_expires_at_idx
    ON arcature_password_resets (expires_at)
