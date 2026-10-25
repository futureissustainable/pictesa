//! Hosted-mode billing: prepaid USD wallets charged when a user runs on the operator's
//! (platform) provider keys. Bring-your-own-key runs are never charged.
//!
//! Configuration (environment):
//! - `PICTESA_BILLING=1` lets hosted users run on platform keys and pay from their wallet.
//! - `PICTESA_MARKUP` multiplies provider cost (default 1.25).
//! - `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET` enable card top-ups.

use hmac::{Hmac, Mac};
use pictesa_core::now_ms;
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;

use crate::{Engine, EngineError, Mode, Result};

pub const TOPUPS_CENTS: &[i64] = &[1000, 2500, 5000];
const MICROS: f64 = 1_000_000.0;

#[derive(Debug, Clone)]
pub struct BillingConfig {
    pub enabled: bool,
    pub markup: f64,
    pub stripe_secret: Option<String>,
    pub stripe_webhook_secret: Option<String>,
}

impl BillingConfig {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self {
            enabled: get("PICTESA_BILLING").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
            markup: get("PICTESA_MARKUP").and_then(|v| v.parse::<f64>().ok()).filter(|m| (1.0..=10.0).contains(m)).unwrap_or(1.25),
            stripe_secret: get("STRIPE_SECRET_KEY"),
            stripe_webhook_secret: get("STRIPE_WEBHOOK_SECRET"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LedgerEntry {
    pub id: String,
    pub amount_usd: f64,
    pub reason: String,
    pub job_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BillingView {
    pub enabled: bool,
    pub topups_available: bool,
    pub balance_usd: f64,
    pub debt_usd: f64,
    pub locked: bool,
    pub markup: f64,
    pub topups_usd: Vec<f64>,
    pub ledger: Vec<LedgerEntry>,
}

pub fn to_micros(usd: f64) -> i64 {
    (usd * MICROS).ceil() as i64
}

fn usd(micros: i64) -> f64 {
    (micros as f64 / MICROS * 10_000.0).round() / 10_000.0
}

impl Engine {
    pub fn billing(&self) -> &BillingConfig {
        &self.inner.billing
    }

    /// True when hosted users may run on platform keys and pay per run.
    pub fn platform_billing(&self) -> bool {
        self.mode() == Mode::Hosted && self.inner.billing.enabled
    }

    pub async fn billing_view(&self, user: &str) -> Result<BillingView> {
        let wallet: Option<(i64, i64, bool)> = sqlx::query_as("SELECT balance_micros, debt_micros, locked FROM wallets WHERE user_id = ?").bind(user).fetch_optional(&self.inner.db).await?;
        let (balance, debt, locked) = wallet.unwrap_or((0, 0, false));
        let rows: Vec<(String, i64, String, Option<String>, i64)> =
            sqlx::query_as("SELECT id, amount_micros, reason, job_id, created_at FROM ledger WHERE user_id = ? ORDER BY created_at DESC LIMIT 100")
                .bind(user)
                .fetch_all(&self.inner.db)
                .await?;
        Ok(BillingView {
            enabled: self.platform_billing(),
            topups_available: self.platform_billing() && self.inner.billing.stripe_secret.is_some() && self.inner.billing.stripe_webhook_secret.is_some(),
            balance_usd: usd(balance),
            debt_usd: usd(debt),
            locked,
            markup: self.inner.billing.markup,
            topups_usd: TOPUPS_CENTS.iter().map(|c| *c as f64 / 100.0).collect(),
            ledger: rows.into_iter().map(|(id, amount, reason, job_id, created_at)| LedgerEntry { id, amount_usd: usd(amount), reason, job_id, created_at }).collect(),
        })
    }

    /// Adds money to a wallet once per idempotency id; money first pays off debt.
    pub(crate) async fn credit(&self, user: &str, id: &str, micros: i64, reason: &str, job: Option<&str>) -> Result<bool> {
        let mut tx = self.inner.db.begin().await?;
        let added = sqlx::query("INSERT OR IGNORE INTO ledger (id, user_id, amount_micros, reason, job_id, created_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(id)
            .bind(user)
            .bind(micros)
            .bind(reason)
            .bind(job)
            .bind(now_ms())
            .execute(&mut *tx)
            .await?;
        if added.rows_affected() == 1 {
            sqlx::query("INSERT OR IGNORE INTO wallets (user_id) VALUES (?)").bind(user).execute(&mut *tx).await?;
            sqlx::query("UPDATE wallets SET balance_micros = balance_micros + max(? - debt_micros, 0), debt_micros = max(debt_micros - ?, 0) WHERE user_id = ?")
                .bind(micros)
                .bind(micros)
                .bind(user)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(job) = job.filter(|job| id == format!("refund:{job}")) {
            sqlx::query("UPDATE spend_runs SET actual_micros = 0 WHERE job_id = ? AND user_id = ?")
                .bind(job).bind(user).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(added.rows_affected() == 1)
    }

    /// Takes money out (refunds after spending turn into debt).
    pub(crate) async fn debit_to_debt(&self, user: &str, id: &str, micros: i64, reason: &str) -> Result<()> {
        let mut tx = self.inner.db.begin().await?;
        let added = sqlx::query("INSERT OR IGNORE INTO ledger (id, user_id, amount_micros, reason, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(id)
            .bind(user)
            .bind(-micros)
            .bind(reason)
            .bind(now_ms())
            .execute(&mut *tx)
            .await?;
        if added.rows_affected() == 1 {
            sqlx::query("INSERT OR IGNORE INTO wallets (user_id) VALUES (?)").bind(user).execute(&mut *tx).await?;
            sqlx::query("UPDATE wallets SET debt_micros = debt_micros + max(? - balance_micros, 0), balance_micros = max(balance_micros - ?, 0) WHERE user_id = ?")
                .bind(micros)
                .bind(micros)
                .bind(user)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Returns a job's charge to the wallet (failed or canceled runs cost nothing).
    pub(crate) async fn refund_job(&self, job_id: &str) {
        // History may be deleted while a failed run is being refunded.
        let row: Option<(String, i64)> = sqlx::query_as("SELECT user_id, COALESCE(actual_micros, 0) FROM spend_runs WHERE job_id = ?").bind(job_id).fetch_optional(&self.inner.db).await.ok().flatten();
        if let Some((user, charged)) = row.filter(|(_, c)| *c > 0) {
            if let Err(e) = self.credit(&user, &format!("refund:{job_id}"), charged, "Refund for a run that did not finish", Some(job_id)).await {
                tracing::error!("refund for {job_id} failed: {e}");
            }
            return;
        }
        if let Err(e) = sqlx::query("UPDATE spend_runs SET actual_micros = 0 WHERE job_id = ?").bind(job_id).execute(&self.inner.db).await {
            tracing::error!("spend refund for {job_id} failed: {e}");
        }
    }

    /// Operator command: grants an allowance by email (idempotent per operation name).
    pub async fn grant(&self, email: &str, usd_amount: f64, operation: &str) -> Result<()> {
        if !(0.01..=10_000.0).contains(&usd_amount) || operation.is_empty() || operation.len() > 80 {
            return Err(EngineError::invalid("Amount must be 0.01 to 10,000 USD and the operation needs a short name.", None));
        }
        let user: Option<(String,)> = sqlx::query_as("SELECT id FROM users WHERE email = ?").bind(email.trim().to_lowercase()).fetch_optional(&self.inner.db).await?;
        let (user,) = user.ok_or_else(|| EngineError::not_found("account"))?;
        let added = self.credit(&user, &format!("grant:{user}:{operation}"), to_micros(usd_amount), "Allowance from the operator", None).await?;
        if !added {
            return Err(EngineError::Conflict(format!("The grant \"{operation}\" was already given to this account.")));
        }
        Ok(())
    }

    /// Starts a Stripe Checkout for a fixed top-up amount and returns its URL.
    pub async fn create_checkout(&self, user: &str, email: Option<&str>, request_id: &str, amount_cents: i64) -> Result<String> {
        let cfg = &self.inner.billing;
        let (Some(secret), Some(_)) = (&cfg.stripe_secret, &cfg.stripe_webhook_secret) else {
            return Err(EngineError::invalid("Top-ups are not available on this server.", None));
        };
        if !self.platform_billing() {
            return Err(EngineError::invalid("This server does not sell credits.", None));
        }
        if !TOPUPS_CENTS.contains(&amount_cents) {
            return Err(EngineError::invalid("Choose one of the offered amounts.", Some("amount_cents")));
        }
        if request_id.len() < 8 || request_id.len() > 64 {
            return Err(EngineError::invalid("Bad request id.", Some("request_id")));
        }
        let base = self.inner.cfg.public_url.clone().ok_or_else(|| EngineError::invalid("The server has no public URL configured.", None))?;
        sqlx::query("INSERT OR IGNORE INTO checkouts (user_id, request_id, amount_cents, created_at) VALUES (?, ?, ?, ?)")
            .bind(user)
            .bind(request_id)
            .bind(amount_cents)
            .bind(now_ms())
            .execute(&self.inner.db)
            .await?;
        let (stored_amount, url): (i64, Option<String>) =
            sqlx::query_as("SELECT amount_cents, url FROM checkouts WHERE user_id = ? AND request_id = ?").bind(user).bind(request_id).fetch_one(&self.inner.db).await?;
        if stored_amount != amount_cents {
            return Err(EngineError::Conflict("Start a new checkout for this amount.".into()));
        }
        if let Some(url) = url {
            return Ok(url);
        }
        let base = base.trim_end_matches('/');
        let mut form = vec![
            ("mode", "payment".to_string()),
            ("line_items[0][price_data][currency]", "usd".into()),
            ("line_items[0][price_data][unit_amount]", amount_cents.to_string()),
            ("line_items[0][price_data][product_data][name]", "Pictesa prepaid generation balance".into()),
            ("line_items[0][quantity]", "1".into()),
            ("metadata[user_id]", user.to_string()),
            ("metadata[request_id]", request_id.to_string()),
            ("success_url", format!("{base}/settings/billing?checkout=success")),
            ("cancel_url", format!("{base}/settings/billing?checkout=canceled")),
        ];
        if let Some(e) = email {
            form.push(("customer_email", e.to_string()));
        }
        let resp = self
            .inner
            .http
            .post("https://api.stripe.com/v1/checkout/sessions")
            .bearer_auth(secret)
            .header("Idempotency-Key", format!("pictesa:{user}:{request_id}"))
            .form(&form)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| EngineError::Internal(anyhow::anyhow!("stripe: {e}")))?;
        if !resp.status().is_success() {
            return Err(EngineError::Internal(anyhow::anyhow!("stripe checkout failed with {}", resp.status())));
        }
        let session: Value = resp.json().await.map_err(|e| EngineError::Internal(e.into()))?;
        let (Some(id), Some(url)) = (session.get("id").and_then(Value::as_str), session.get("url").and_then(Value::as_str)) else {
            return Err(EngineError::Internal(anyhow::anyhow!("stripe returned no session")));
        };
        if !url.starts_with("https://checkout.stripe.com/") {
            return Err(EngineError::Internal(anyhow::anyhow!("unexpected checkout URL")));
        }
        sqlx::query("UPDATE checkouts SET session_id = ?, url = ? WHERE user_id = ? AND request_id = ?").bind(id).bind(url).bind(user).bind(request_id).execute(&self.inner.db).await?;
        Ok(url.to_string())
    }

    /// Verifies and applies a Stripe webhook (paid checkouts, refunds, disputes).
    pub async fn apply_stripe_event(&self, body: &[u8], signature: &str) -> Result<()> {
        let secret = self.inner.billing.stripe_webhook_secret.as_deref().ok_or_else(|| EngineError::invalid("Billing is not configured.", None))?;
        if !verify_stripe(body, signature, secret, now_ms() / 1000) {
            return Err(EngineError::Unauthorized("Invalid Stripe signature.".into()));
        }
        let event: Value = serde_json::from_slice(body).map_err(|_| EngineError::invalid("Invalid event.", None))?;
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let obj = event.pointer("/data/object").cloned().unwrap_or(Value::Null);
        match kind {
            "checkout.session.completed" | "checkout.session.async_payment_succeeded" if obj.get("payment_status").and_then(Value::as_str) == Some("paid") => {
                let user = obj.pointer("/metadata/user_id").and_then(Value::as_str).ok_or_else(|| EngineError::invalid("missing user", None))?;
                let request = obj.pointer("/metadata/request_id").and_then(Value::as_str).ok_or_else(|| EngineError::invalid("missing request", None))?;
                let session = obj.get("id").and_then(Value::as_str).unwrap_or("");
                let total = obj.get("amount_total").and_then(Value::as_i64).unwrap_or(0);
                if obj.get("currency").and_then(Value::as_str) != Some("usd") {
                    return Err(EngineError::invalid("Payment currency mismatch.", None));
                }
                let row: Option<(i64, Option<String>)> =
                    sqlx::query_as("SELECT amount_cents, session_id FROM checkouts WHERE user_id = ? AND request_id = ?").bind(user).bind(request).fetch_optional(&self.inner.db).await?;
                let Some((amount, stored_session)) = row else { return Err(EngineError::invalid("Unknown checkout.", None)) };
                if amount != total || stored_session.as_deref().is_some_and(|s| s != session) {
                    return Err(EngineError::invalid("Payment mismatch.", None));
                }
                let micros = amount * 10_000;
                let mut tx = self.inner.db.begin().await?;
                let added = sqlx::query("INSERT OR IGNORE INTO ledger (id, user_id, amount_micros, reason, created_at) VALUES (?, ?, ?, 'Balance top-up', ?)")
                    .bind(format!("stripe:{session}"))
                    .bind(user)
                    .bind(micros)
                    .bind(now_ms())
                    .execute(&mut *tx)
                    .await?;
                if added.rows_affected() == 1 {
                    sqlx::query("INSERT OR IGNORE INTO wallets (user_id) VALUES (?)").bind(user).execute(&mut *tx).await?;
                    sqlx::query("UPDATE wallets SET balance_micros = balance_micros + max(? - debt_micros, 0), debt_micros = max(debt_micros - ?, 0) WHERE user_id = ?")
                        .bind(micros)
                        .bind(micros)
                        .bind(user)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query("UPDATE checkouts SET paid = 1, session_id = ?, payment_intent = ? WHERE user_id = ? AND request_id = ?")
                    .bind(session)
                    .bind(obj.get("payment_intent").and_then(Value::as_str))
                    .bind(user)
                    .bind(request)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
            "charge.refunded" => {
                let intent = obj.get("payment_intent").and_then(Value::as_str).unwrap_or("");
                let refunded = obj.get("amount_refunded").and_then(Value::as_i64).unwrap_or(0);
                let row: Option<(String, i64, i64)> = sqlx::query_as("SELECT user_id, amount_cents, refunded_cents FROM checkouts WHERE payment_intent = ? AND paid = 1")
                    .bind(intent)
                    .fetch_optional(&self.inner.db)
                    .await?;
                let Some((user, amount, already)) = row else { return Err(EngineError::Conflict("The payment has not been recorded yet.".into())) };
                let delta = refunded.min(amount) - already;
                if delta > 0 {
                    self.debit_to_debt(&user, &format!("stripe-refund:{intent}:{refunded}"), delta * 10_000, "Payment refund").await?;
                    sqlx::query("UPDATE checkouts SET refunded_cents = ? WHERE payment_intent = ?").bind(refunded.min(amount)).bind(intent).execute(&self.inner.db).await?;
                }
            }
            "charge.dispute.created" => {
                if let Some(intent) = obj.get("payment_intent").and_then(Value::as_str) {
                    sqlx::query("INSERT OR IGNORE INTO wallets (user_id) SELECT user_id FROM checkouts WHERE payment_intent = ?").bind(intent).execute(&self.inner.db).await?;
                    sqlx::query("UPDATE wallets SET locked = 1 WHERE user_id = (SELECT user_id FROM checkouts WHERE payment_intent = ?)").bind(intent).execute(&self.inner.db).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Checks a `Stripe-Signature` header (`t=...,v1=...`) within a five minute window.
pub fn verify_stripe(body: &[u8], header: &str, secret: &str, now_secs: i64) -> bool {
    let mut t = None;
    let mut sigs = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", v)) => t = v.parse::<i64>().ok(),
            Some(("v1", v)) => sigs.push(v.to_string()),
            _ => {}
        }
    }
    let Some(t) = t else { return false };
    if (now_secs - t).abs() > 300 || sigs.is_empty() {
        return false;
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(format!("{t}.").as_bytes());
    mac.update(body);
    let expected: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
    sigs.iter().any(|s| s.len() == expected.len() && s.bytes().zip(expected.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refund_survives_deleted_job_history_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("pictesa-refund-deleted-{}", crate::new_id()));
        let e = Engine::open(crate::Config::local(dir)).await.unwrap();
        e.credit(crate::LOCAL_USER, "initial", 500_000, "Remaining balance", None).await.unwrap();
        sqlx::query("INSERT INTO spend_runs (job_id,user_id,model,provider,estimated_micros,actual_micros,created_at) VALUES ('deleted','local','model','provider',400000,500000,?)")
            .bind(now_ms()).execute(e.db()).await.unwrap();
        e.refund_job("deleted").await;
        e.refund_job("deleted").await;
        assert_eq!(e.billing_view(crate::LOCAL_USER).await.unwrap().balance_usd, 1.0);
        assert_eq!(e.spend_view(crate::LOCAL_USER).await.unwrap().today_usd, 0.0);
    }

    #[test]
    fn stripe_signature_roundtrip() {
        let body = br#"{"type":"ping"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_test").unwrap();
        mac.update(b"1000.");
        mac.update(body);
        let sig: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert!(verify_stripe(body, &format!("t=1000,v1={sig}"), "whsec_test", 1100));
        assert!(!verify_stripe(body, &format!("t=1000,v1={sig}"), "whsec_test", 2000), "outside the window");
        assert!(!verify_stripe(body, &format!("t=1000,v1={sig}"), "whsec_other", 1100), "wrong secret");
    }
}
