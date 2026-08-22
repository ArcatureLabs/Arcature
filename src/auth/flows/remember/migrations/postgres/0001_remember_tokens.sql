-- 0001_remember_tokens.sql (PostgreSQL) -- rotating remember-me tokens.
--
-- One table, arcature_remember_tokens, holding one row per remembered device.
-- Per *device*, not per subject: signing in on a phone must not sign anybody
-- out of a laptop, which is the one structural difference between this table
-- and arcature_password_resets and the reason issuing does not clear the
-- subject's other rows.
--
-- Statements are separated by a line reading `--;;` and executed one at a
-- time, so the file never relies on multi-statement support.
--
-- `series` is the *public* half: 16 random bytes that identify one device's
-- chain of tokens and survive every rotation. It is the primary key because
-- every read is a lookup by series, and it is stable because a rotating
-- credential with nothing stable about it has nothing to attach a theft report
-- to -- the presented secret is wrong precisely when there is something to
-- report, so it cannot be the handle.
--
-- `secret_digest` is the SHA-256 of the secret half, which is never stored.
-- The reasoning is `src/tokens/`'s and `arcature_password_resets`'s: a backup,
-- a replica, or a `SELECT` by a reporting account yields 32 bytes that cannot
-- be turned back into a cookie. SHA-256 and deliberately not argon2, because
-- the secret is 256 bits of uniform randomness and a slow hash defends against
-- nothing it was designed to defend against -- and here there is a second
-- reason: this digest is computed on ordinary authenticated requests, so a
-- deliberately slow hash would be a self-inflicted denial of service.
--
-- `previous_digest` and `rotated_at` are what make rotation survivable rather
-- than merely correct. Every accepted cookie is replaced, so a browser that
-- issues two requests with the same cookie -- restoring twenty tabs, retrying
-- a dropped response -- would otherwise present a retired secret and be
-- indistinguishable from a thief. Keeping the previous digest, and the instant
-- it was retired, lets the store accept it for a short grace window and treat
-- it as theft afterwards. Both are NULL for a token that has never rotated,
-- and the store writes them together.
--
-- `expires_at` is NOT NULL and there is no "never expires". Every read carries
-- `expires_at > now()`, so a cookie stops working the moment it lapses whether
-- or not the sweep has run. This is the credential that signs somebody in
-- weeks after their session ended; an unbounded one is a password that never
-- changes and that its owner does not know they have.

CREATE TABLE IF NOT EXISTS arcature_remember_tokens (
    series          BYTEA       PRIMARY KEY,
    secret_digest   BYTEA       NOT NULL,
    previous_digest BYTEA,
    rotated_at      TIMESTAMPTZ,
    subject         TEXT        NOT NULL,
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
)
--;;
-- "Sign out everywhere", and the theft cascade. Both are `WHERE subject = $1`,
-- and the cascade runs at the worst possible moment -- somebody's credential
-- has just been used by somebody else -- so it is worth an index.
CREATE INDEX IF NOT EXISTS arcature_remember_tokens_subject_idx
    ON arcature_remember_tokens (subject)
--;;
-- The sweep path: lapsed rows.
CREATE INDEX IF NOT EXISTS arcature_remember_tokens_expires_at_idx
    ON arcature_remember_tokens (expires_at)
