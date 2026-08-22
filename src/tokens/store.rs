//! The store itself: mint, read, list, revoke, sweep.

use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::types::Json;

use super::dialect::{TokenDb, TokenPool, restored_time, sql, stored_time};
use super::error::ApiTokenError;
use super::migrate;
use super::token::{ID_BYTES, IssuedApiToken, PlaintextToken, SECRET_BYTES, format_plaintext};
use super::{Abilities, ApiToken, ApiTokenId, NewApiToken};

/// The row type of the dialect this build speaks.
type TokenRow = <TokenDb as sqlx::Database>::Row;

/// How many fresh ids [`ApiTokens::issue`] will try before giving up.
///
/// An id is 128 bits, so one clash is already a once-in-the-heat-death event
/// and eight in a row is not chance -- it is a random source that is not
/// random. Looping forever would turn that into a hang; reporting it turns it
/// into a log line someone can act on.
const ISSUE_ATTEMPTS: u32 = 8;

/// Hashed personal access tokens in the application's own database.
///
/// # What is stored
///
/// A token is two halves: a 16-byte public id and a 32-byte secret. The row
/// holds the id in the clear -- it is a lookup key, not a credential -- and
/// the SHA-256 of the secret. The secret itself is never written anywhere.
/// [`issue`](Self::issue) hands back the only copy of the plaintext; after
/// that call returns there is no way, from the database or from this crate,
/// to produce it again.
///
/// That is the property the whole design exists for. A stolen backup, a
/// compromised read replica, or a reporting account with `SELECT` on the
/// table yields 32 bytes of digest per token and no way to authenticate as
/// anybody.
///
/// # Why SHA-256 and not argon2
///
/// See the comment on `digest_of` at the bottom of this file. In one line: the secret is 256 bits of
/// uniform randomness, so a slow hash defends against nothing, and running
/// one per API request is a denial of service the application inflicts on
/// itself.
///
/// # Why expiry is in the query
///
/// Every read carries `expires_at > now()`, evaluated by the database. An
/// expired token is invisible from the instant it expires, whether or not
/// [`sweep_expired`](Self::sweep_expired) has run -- so a sweep that is late,
/// misconfigured, or never wired up costs disk, not security.
///
/// # Example
///
/// ```no_run
/// // Needs a database, so this example is compiled and not run.
/// use arcature::tokens::{Abilities, ApiTokens, NewApiToken};
/// use std::time::Duration;
///
/// # async fn example(pool: arcature::tokens::TokenPool)
/// # -> Result<(), Box<dyn std::error::Error>> {
/// let tokens = ApiTokens::new(pool);
/// tokens.migrate().await?;
///
/// let issued = tokens
///     .issue(
///         &NewApiToken::expiring_in("user:42", "laptop", Duration::from_secs(30 * 86_400))
///             .abilities(Abilities::of(["posts:read", "posts:write"])),
///     )
///     .await?;
///
/// // The one and only time the plaintext exists.
/// let secret = issued.plaintext().expose().to_owned();
/// let id = issued.token().id();
///
/// // Afterwards the store knows the record but not the secret.
/// let stored = tokens.find(id).await?.expect("just issued");
/// assert!(stored.can("posts:write"));
///
/// tokens.revoke(id).await?;
/// assert!(tokens.find(id).await?.is_none());
/// # let _ = secret;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ApiTokens {
    pool: TokenPool,
}

impl ApiTokens {
    /// Build a store over an existing pool.
    ///
    /// There is deliberately no `connect_lazy` twin here as there is on
    /// [`DbSessionStore`](crate::auth::session_store::DbSessionStore). That
    /// one exists because the session layer is configured before the
    /// framework's pool exists; a token store is used from handlers, by which
    /// time the application's pool is in hand, and a second pool would be a
    /// second slice of the database's connection budget for no reason.
    #[must_use]
    pub fn new(pool: TokenPool) -> Self {
        Self { pool }
    }

    /// The pool the store runs over.
    #[must_use]
    pub fn pool(&self) -> &TokenPool {
        &self.pool
    }

    /// Create `arcature_api_tokens` and its indexes if they are not there.
    ///
    /// Idempotent, and safe to run from every replica at once: the migration
    /// is applied under the dialect's advisory lock with a history table.
    /// Call it at startup. A store whose table is missing fails on the first
    /// request instead, which is the same outage discovered by a user.
    ///
    /// # Errors
    ///
    /// Returns [`ApiTokenError::Database`] if the database is unreachable or
    /// rejects a statement.
    pub async fn migrate(&self) -> Result<(), ApiTokenError> {
        migrate::apply(&self.pool).await
    }

    /// Mint a token, write its digest, and return the plaintext once.
    ///
    /// # Errors
    ///
    /// * [`ApiTokenError::Entropy`] if the OS randomness source is
    ///   unavailable. No fallback is attempted: a token that is merely hard to
    ///   predict is not a secret.
    /// * [`ApiTokenError::IdCollision`] if [`ISSUE_ATTEMPTS`] random ids were
    ///   all taken, which in practice means the randomness source is broken.
    /// * [`ApiTokenError::Database`] if the database rejects the insert.
    pub async fn issue(&self, request: &NewApiToken) -> Result<IssuedApiToken, ApiTokenError> {
        let mut secret = [0u8; SECRET_BYTES];
        fill_random(&mut secret)?;
        let digest = digest_of(&secret);

        let abilities = request.abilities_ref().clone();
        // Bound explicitly rather than left to the column default, so the
        // value written is the one this process can also return without a
        // second round trip to read it back.
        let created_at = Utc::now();
        let expires_at = request.expires_at();

        for _ in 0..ISSUE_ATTEMPTS {
            let mut id_bytes = [0u8; ID_BYTES];
            fill_random(&mut id_bytes)?;

            let written = sqlx::query(sql::INSERT_NEW)
                .bind(id_bytes.to_vec())
                .bind(digest.to_vec())
                .bind(request.tokenable_id())
                .bind(request.name())
                .bind(Json(abilities.as_slice()))
                .bind(stored_time(expires_at))
                .bind(stored_time(created_at))
                .execute(&self.pool)
                .await?
                .rows_affected();

            if written == 0 {
                // `DO NOTHING` / `INSERT IGNORE`: the id is taken. Draw
                // another rather than parsing a driver-specific constraint
                // name out of an error.
                continue;
            }

            let id = ApiTokenId::from_bytes(id_bytes);
            let plaintext = PlaintextToken::new(format_plaintext(&id_bytes, &secret));
            let token = ApiToken::from_row(
                id,
                request.tokenable_id().to_owned(),
                request.name().to_owned(),
                abilities,
                expires_at,
                created_at,
            );
            return Ok(IssuedApiToken::new(token, plaintext));
        }

        Err(ApiTokenError::IdCollision {
            attempts: ISSUE_ATTEMPTS,
        })
    }

    /// Read one live token by its public id.
    ///
    /// This is a lookup, not an authentication: it proves nothing about who
    /// is asking, because the id is not a secret. Use it to render a token
    /// management screen or to check that a revocation target exists.
    ///
    /// # Errors
    ///
    /// Returns [`ApiTokenError::Database`] if the query fails, or
    /// [`ApiTokenError::Decode`] / [`ApiTokenError::Expiry`] if a row does not
    /// hold what the schema promises.
    pub async fn find(&self, id: ApiTokenId) -> Result<Option<ApiToken>, ApiTokenError> {
        let Some(row) = sqlx::query(sql::FIND)
            .bind(id.as_bytes().to_vec())
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };

        // Columns by index, not by name: the three dialects agree on the
        // order the statement asks for and nothing else has to be true.
        let tokenable_id: String = row.try_get(0)?;
        let name: String = row.try_get(1)?;
        let abilities = abilities_at(&row, 2)?;
        let expires_at = restored_time(row.try_get(3)?)?;
        let created_at = restored_time(row.try_get(4)?)?;

        Ok(Some(ApiToken::from_row(
            id,
            tokenable_id,
            name,
            abilities,
            expires_at,
            created_at,
        )))
    }

    /// Every live token issued to one subject, newest first.
    ///
    /// The plaintexts are not here and cannot be; this is the list a "your API
    /// tokens" screen renders, with names, abilities, and dates.
    ///
    /// # Errors
    ///
    /// Returns [`ApiTokenError::Database`] if the query fails, or
    /// [`ApiTokenError::Decode`] / [`ApiTokenError::Expiry`] if a row does not
    /// hold what the schema promises.
    pub async fn list_for(&self, tokenable_id: &str) -> Result<Vec<ApiToken>, ApiTokenError> {
        let rows = sqlx::query(sql::LIST_FOR)
            .bind(tokenable_id)
            .fetch_all(&self.pool)
            .await?;

        let mut tokens = Vec::with_capacity(rows.len());
        for row in rows {
            let raw_id: Vec<u8> = row.try_get(0)?;
            let id_bytes: [u8; ID_BYTES] = raw_id.as_slice().try_into().map_err(|_| {
                ApiTokenError::Decode(format!(
                    "id column holds {} bytes, expected {ID_BYTES}",
                    raw_id.len()
                ))
            })?;
            let tokenable_id: String = row.try_get(1)?;
            let name: String = row.try_get(2)?;
            let abilities = abilities_at(&row, 3)?;
            let expires_at = restored_time(row.try_get(4)?)?;
            let created_at = restored_time(row.try_get(5)?)?;

            tokens.push(ApiToken::from_row(
                ApiTokenId::from_bytes(id_bytes),
                tokenable_id,
                name,
                abilities,
                expires_at,
                created_at,
            ));
        }
        Ok(tokens)
    }

    /// Revoke one token, reporting whether there was one to revoke.
    ///
    /// Revocation is a delete, not a flag. A revoked row that is still in the
    /// table is a row some future query can forget to filter; a row that is
    /// gone cannot authenticate anybody by accident.
    ///
    /// # Errors
    ///
    /// Returns [`ApiTokenError::Database`] if the statement fails.
    pub async fn revoke(&self, id: ApiTokenId) -> Result<bool, ApiTokenError> {
        let result = sqlx::query(sql::DELETE)
            .bind(id.as_bytes().to_vec())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Revoke every token belonging to one subject, and report how many.
    ///
    /// This is the "sign out everywhere" and "the laptop was stolen" path,
    /// and the one to call when a password changes.
    ///
    /// # Errors
    ///
    /// Returns [`ApiTokenError::Database`] if the statement fails.
    pub async fn revoke_all_for(&self, tokenable_id: &str) -> Result<u64, ApiTokenError> {
        let result = sqlx::query(sql::DELETE_FOR)
            .bind(tokenable_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete every token whose expiry has passed, and report how many.
    ///
    /// This reclaims disk. It is not what makes expiry correct -- every read
    /// already carries the expiry predicate -- so a deployment that never
    /// calls it is secure, merely wasteful.
    ///
    /// # Errors
    ///
    /// Returns [`ApiTokenError::Database`] if the statement fails.
    pub async fn sweep_expired(&self) -> Result<u64, ApiTokenError> {
        let result = sqlx::query(sql::DELETE_EXPIRED).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }
}

/// The 32 bytes a token's secret half is stored as.
///
/// # Why this is SHA-256 and not argon2, bcrypt, or scrypt
///
/// This looks like the password path and is not, and the difference decides
/// the algorithm.
///
/// A password hash is slow on purpose because a password is *low entropy*.
/// Users pick from a distribution an attacker can enumerate -- a wordlist, a
/// leaked corpus, a few billion candidates -- so the only defence a stolen
/// hash has is that each guess costs real time and memory. Argon2 buys that
/// time. It is the right call in [`crate::auth::password`], and it stays the
/// right call there.
///
/// A token secret is [`SECRET_BYTES`] bytes straight from the OS CSPRNG: 256
/// bits of uniform randomness, with no distribution to guess from. Enumerating
/// it is not expensive, it is impossible -- 2^255 expected attempts against a
/// keyspace with no structure, no dictionary, and no human habits in it.
/// Multiplying an impossible search by argon2's cost factor leaves it
/// impossible. **The slow hash buys nothing, because there was nothing to buy:
/// the entropy already did the work a slow hash exists to do.**
///
/// The cost, meanwhile, is real and lands on every request. A token is
/// presented on *each* API call, so verification is on the hot path in a way a
/// login never is. Argon2's default parameters are deliberately expensive --
/// tens of milliseconds and tens of megabytes, tuned so that an attacker's
/// GPU farm is slow. Put that in front of every request and the tuning applies
/// to the server: a client with a valid token and a loop becomes a memory-hard
/// workload generator, and a handful of concurrent requests exhausts the very
/// resource the parameters were chosen to make scarce. Rate limiting does not
/// save it either, because the work happens before the request is known to be
/// abusive. **Choosing argon2 here would not harden the token; it would hand
/// anyone holding one a denial-of-service primitive against the application.**
///
/// So: a single SHA-256, which is fast, which has no length-extension risk in
/// this construction (the input is a fixed 32 bytes and the digest is never a
/// prefix of a longer authenticated message), and which is compared in
/// constant time so the comparison itself leaks nothing.
///
/// The reasoning generalises: hash slowly what humans chose, hash quickly what
/// the CSPRNG chose. The same conclusion is why
/// [`DbSessionStore`](crate::auth::session_store::DbSessionStore) stores a
/// SHA-256 of a session id.
pub(crate) fn digest_of(secret: &[u8]) -> [u8; 32] {
    Sha256::digest(secret).into()
}

/// Fill a buffer from the OS randomness source.
///
/// No fallback. If the OS cannot produce randomness the honest outcome is an
/// error the operator sees, not a token minted from a clock.
fn fill_random(buffer: &mut [u8]) -> Result<(), ApiTokenError> {
    getrandom::fill(buffer).map_err(|_| ApiTokenError::Entropy)
}

/// Decode the abilities column of a row.
///
/// The column is JSONB on PostgreSQL, JSON on MySQL, and TEXT on SQLite;
/// `sqlx::types::Json` covers all three, so the store has one code path.
fn abilities_at(row: &TokenRow, index: usize) -> Result<Abilities, ApiTokenError> {
    let stored: Json<Vec<String>> = row
        .try_get(index)
        .map_err(|error| ApiTokenError::Decode(error.to_string()))?;
    Ok(Abilities::of(stored.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_not_the_secret() {
        // The property the schema rests on: what goes into the row is not
        // what the caller holds, and is a fixed 32 bytes whatever the input.
        let secret = [0xa5u8; SECRET_BYTES];
        let digest = digest_of(&secret);
        assert_eq!(digest.len(), 32);
        assert_ne!(&digest[..], &secret[..]);
    }

    #[test]
    fn the_digest_is_the_documented_sha256() {
        // Pinned against an independently known SHA-256 so a future change of
        // algorithm cannot pass unnoticed: this is the digest of 32 zero
        // bytes.
        let digest = super::super::token::hex_encode(&digest_of(&[0u8; 32]));
        assert_eq!(
            digest,
            "66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925"
        );
    }

    #[test]
    fn two_secrets_that_differ_in_one_bit_give_different_digests() {
        let a = [0u8; SECRET_BYTES];
        let mut b = [0u8; SECRET_BYTES];
        b[SECRET_BYTES - 1] = 1;
        assert_ne!(digest_of(&a), digest_of(&b));
        // And the same input always gives the same digest, which is what
        // makes a lookup possible at all.
        assert_eq!(digest_of(&a), digest_of(&[0u8; SECRET_BYTES]));
    }

    #[test]
    fn the_random_source_fills_the_whole_buffer() {
        // Not a randomness test -- it is a wiring test. A buffer left
        // untouched by a silently failing call would mint every token with
        // the same all-zero secret.
        let mut first = [0u8; SECRET_BYTES];
        let mut second = [0u8; SECRET_BYTES];
        fill_random(&mut first).expect("the OS randomness source is available");
        fill_random(&mut second).expect("the OS randomness source is available");
        assert_ne!(first, [0u8; SECRET_BYTES]);
        assert_ne!(first, second);
    }
}
