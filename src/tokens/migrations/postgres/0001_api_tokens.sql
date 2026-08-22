-- 0001_api_tokens.sql (PostgreSQL) -- hashed personal access tokens.
--
-- One table, arcature_api_tokens, holding one row per live token.
--
-- Statements are separated by a line reading `--;;` and executed one at a
-- time, so the file never relies on multi-statement support.
--
-- `id` is the *public* half of a token: 16 random bytes that travel in the
-- Authorization header in the clear and are what a lookup indexes on. It is
-- not a secret and is not treated as one.
--
-- `secret_digest` is the SHA-256 of the *secret* half, and the secret half is
-- never stored anywhere. The plaintext is returned once, at creation, and is
-- unrecoverable afterwards: a backup, a replica, or a `SELECT` by a read-only
-- reporting account yields 32 bytes that cannot be turned back into a
-- credential.
--
-- The digest is SHA-256 and deliberately not argon2 or bcrypt. See the
-- comment at the hashing site in `src/tokens/store.rs` -- the short version is
-- that the secret is 256 bits of uniform randomness, so a slow hash defends
-- against nothing, while running one on every API request is a denial of
-- service the application inflicts on itself.
--
-- `expires_at` is not decoration and is NOT NULL. Every read carries
-- `expires_at > now()`, so an expired token stops working the moment it
-- expires whether or not the sweep has run. There is no "never expires":
-- a token that outlives the reason it was minted is the single most common
-- way a leaked credential stays useful, so the column has no null state to
-- mean it.

CREATE TABLE IF NOT EXISTS arcature_api_tokens (
    id            BYTEA       PRIMARY KEY,
    secret_digest BYTEA       NOT NULL,
    tokenable_id  TEXT        NOT NULL,
    name          TEXT        NOT NULL,
    abilities     JSONB       NOT NULL,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
)
--;;
-- Listing and mass revocation: every token belonging to one subject.
CREATE INDEX IF NOT EXISTS arcature_api_tokens_tokenable_idx
    ON arcature_api_tokens (tokenable_id)
--;;
-- The sweep path: expired rows, oldest first.
CREATE INDEX IF NOT EXISTS arcature_api_tokens_expires_at_idx
    ON arcature_api_tokens (expires_at)
