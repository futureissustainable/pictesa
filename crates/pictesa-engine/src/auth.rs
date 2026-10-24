//! Accounts for hosted mode: password sign-up, sessions and personal API tokens.
//! Local mode skips all of this and acts as the single `local` user.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use pictesa_core::now_ms;
use rand::RngCore;

use crate::{Engine, EngineError, NewToken, Result, TokenInfo, User};

pub const SESSION_DAYS: i64 = 30;

fn random_secret(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn digest(secret: &str) -> String {
    blake3::hash(secret.as_bytes()).to_hex().to_string()
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    email: Option<String>,
    name: Option<String>,
    password_hash: Option<String>,
    role: String,
    created_at: i64,
}

impl UserRow {
    fn user(self) -> User {
        User { id: self.id, email: self.email, name: self.name, role: self.role, created_at: self.created_at }
    }
}

impl Engine {
    pub async fn signup(&self, email: &str, password: &str, name: Option<&str>) -> Result<User> {
        let email = email.trim().to_lowercase();
        if !(email.contains('@') && email.len() <= 254 && email.split('@').nth(1).is_some_and(|d| d.contains('.'))) {
            return Err(EngineError::invalid("Enter a valid email address.", Some("email")));
        }
        if password.chars().count() < 10 {
            return Err(EngineError::invalid("Use a password of at least 10 characters.", Some("password")));
        }
        if password.len() > 256 {
            return Err(EngineError::invalid("Use a password of at most 256 characters.", Some("password")));
        }
        let (taken,): (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE email = ?").bind(&email).fetch_one(&self.inner.db).await?;
        if taken > 0 {
            return Err(EngineError::Conflict("An account with this email already exists.".into()));
        }
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
            .map_err(|e| EngineError::Internal(anyhow::anyhow!("password hashing failed: {e}")))?
            .to_string();
        // The first account on a hosted instance administers it; decided inside the insert so two
        // simultaneous first sign-ups cannot both become admin.
        let id = crate::new_id();
        let inserted = sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, role, created_at)
             SELECT ?, ?, ?, ?, CASE WHEN (SELECT count(*) FROM users WHERE id != 'local') = 0 THEN 'admin' ELSE 'user' END, ?",
        )
        .bind(&id)
        .bind(&email)
        .bind(name.map(str::trim).filter(|n| !n.is_empty()))
        .bind(hash)
        .bind(now_ms())
        .execute(&self.inner.db)
        .await;
        if let Err(sqlx::Error::Database(e)) = &inserted {
            if e.is_unique_violation() {
                return Err(EngineError::Conflict("An account with this email already exists.".into()));
            }
        }
        inserted?;
        self.user(&id).await
    }

    pub async fn user(&self, id: &str) -> Result<User> {
        let row: UserRow = sqlx::query_as("SELECT id, email, name, password_hash, role, created_at FROM users WHERE id = ?").bind(id).fetch_one(&self.inner.db).await?;
        Ok(row.user())
    }

    /// Checks the password and returns the user. The same error covers unknown email and wrong password.
    pub async fn login(&self, email: &str, password: &str) -> Result<User> {
        if password.len() > 256 {
            return Err(EngineError::Unauthorized("Email or password is wrong.".into()));
        }
        let email = email.trim().to_lowercase();
        let row: Option<UserRow> = sqlx::query_as("SELECT id, email, name, password_hash, role, created_at FROM users WHERE email = ?")
            .bind(&email)
            .fetch_optional(&self.inner.db)
            .await?;
        let bad = || EngineError::Unauthorized("Email or password is wrong.".into());
        let row = row.ok_or_else(bad)?;
        let stored = row.password_hash.clone().ok_or_else(bad)?;
        let parsed = PasswordHash::new(&stored).map_err(|_| bad())?;
        Argon2::default().verify_password(password.as_bytes(), &parsed).map_err(|_| bad())?;
        Ok(row.user())
    }

    /// Creates a session and returns the cookie value.
    pub async fn create_session(&self, user_id: &str) -> Result<String> {
        let secret = random_secret("");
        let now = now_ms();
        sqlx::query("INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
            .bind(digest(&secret))
            .bind(user_id)
            .bind(now)
            .bind(now + SESSION_DAYS * 86_400_000)
            .execute(&self.inner.db)
            .await?;
        Ok(secret)
    }

    pub async fn session_user(&self, cookie: &str) -> Result<Option<User>> {
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT u.id, u.email, u.name, u.password_hash, u.role, u.created_at FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.id = ? AND s.expires_at > ?",
        )
        .bind(digest(cookie))
        .bind(now_ms())
        .fetch_optional(&self.inner.db)
        .await?;
        Ok(row.map(UserRow::user))
    }

    pub async fn end_session(&self, cookie: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?").bind(digest(cookie)).execute(&self.inner.db).await?;
        Ok(())
    }

    pub async fn create_token(&self, user_id: &str, name: &str) -> Result<NewToken> {
        let name = name.trim();
        if name.is_empty() || name.len() > 80 {
            return Err(EngineError::invalid("Give the token a short name.", Some("name")));
        }
        let token = random_secret("pct_");
        let id = crate::new_id();
        sqlx::query("INSERT INTO api_tokens (id, user_id, name, token_hash, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(user_id)
            .bind(name)
            .bind(digest(&token))
            .bind(now_ms())
            .execute(&self.inner.db)
            .await?;
        Ok(NewToken { id, name: name.to_string(), token })
    }

    pub async fn token_user(&self, token: &str) -> Result<Option<User>> {
        let hash = digest(token);
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT u.id, u.email, u.name, u.password_hash, u.role, u.created_at FROM api_tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(&self.inner.db)
        .await?;
        if row.is_some() {
            let _ = sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE token_hash = ?").bind(now_ms()).bind(&hash).execute(&self.inner.db).await;
        }
        Ok(row.map(UserRow::user))
    }

    pub async fn tokens(&self, user_id: &str) -> Result<Vec<TokenInfo>> {
        let rows: Vec<(String, String, i64, Option<i64>)> =
            sqlx::query_as("SELECT id, name, created_at, last_used_at FROM api_tokens WHERE user_id = ? ORDER BY created_at DESC")
                .bind(user_id)
                .fetch_all(&self.inner.db)
                .await?;
        Ok(rows.into_iter().map(|(id, name, created_at, last_used_at)| TokenInfo { id, name, created_at, last_used_at }).collect())
    }

    pub async fn delete_token(&self, user_id: &str, id: &str) -> Result<()> {
        let done = sqlx::query("DELETE FROM api_tokens WHERE id = ? AND user_id = ?").bind(id).bind(user_id).execute(&self.inner.db).await?;
        if done.rows_affected() == 0 {
            return Err(EngineError::not_found("token"));
        }
        Ok(())
    }
}
