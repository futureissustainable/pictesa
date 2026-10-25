//! Spend includes queued reservations and known charges, net of refunded runs.
//! Calendar boundaries use UTC so caps stay identical across clients.
use pictesa_core::now_ms;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection};

use crate::{billing::to_micros, Engine, EngineError, Result};

const DAY: i64 = 86_400_000;

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct BudgetInput {
    pub daily_usd: Option<f64>,
    pub monthly_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct BudgetProgress {
    pub cap_usd: Option<f64>,
    pub spent_usd: f64,
    pub remaining_usd: Option<f64>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct SpendGroup {
    pub name: String,
    pub usd: f64,
}

#[derive(Debug, Serialize)]
pub struct SpendView {
    pub today_usd: f64,
    pub seven_days_usd: f64,
    pub thirty_days_usd: f64,
    pub days: Vec<SpendGroup>,
    pub models: Vec<SpendGroup>,
    pub providers: Vec<SpendGroup>,
    pub daily: BudgetProgress,
    pub monthly: BudgetProgress,
    pub unknown_runs: i64,
    pub estimated_runs: i64,
    pub timezone: &'static str,
}

fn progress(cap: Option<i64>, spent: i64) -> BudgetProgress {
    BudgetProgress { cap_usd: cap.map(|v| v as f64 / 1e6), spent_usd: spent as f64 / 1e6, remaining_usd: cap.map(|v| (v - spent).max(0) as f64 / 1e6) }
}

async fn boundaries(db: &mut SqliteConnection, now: i64) -> Result<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT CAST(strftime('%s', ? / 1000, 'unixepoch', 'start of day') AS INTEGER) * 1000,
        CAST(strftime('%s', ? / 1000, 'unixepoch', 'start of month') AS INTEGER) * 1000",
    )
    .bind(now)
    .bind(now)
    .fetch_one(db)
    .await?)
}

async fn total(db: &mut SqliteConnection, user: &str, since: i64) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COALESCE(sum(COALESCE(actual_micros, estimated_micros, 0)), 0) FROM spend_runs WHERE user_id = ? AND created_at >= ?")
        .bind(user)
        .bind(since)
        .fetch_one(db)
        .await?)
}

impl Engine {
    pub async fn set_budgets(&self, user: &str, input: BudgetInput) -> Result<SpendView> {
        for cap in [input.daily_usd, input.monthly_usd].into_iter().flatten() {
            if !cap.is_finite() || !(0.0..=1_000_000.0).contains(&cap) {
                return Err(EngineError::invalid("Budget caps must be between $0 and $1,000,000, or empty for no cap.", Some("budget")));
            }
        }
        sqlx::query(
            "INSERT INTO budgets (user_id, daily_micros, monthly_micros) VALUES (?, ?, ?)
            ON CONFLICT(user_id) DO UPDATE SET daily_micros = excluded.daily_micros, monthly_micros = excluded.monthly_micros",
        )
        .bind(user)
        .bind(input.daily_usd.map(to_micros))
        .bind(input.monthly_usd.map(to_micros))
        .execute(&self.inner.db)
        .await?;
        self.spend_view(user).await
    }

    /// The caller holds BEGIN IMMEDIATE through inserting every run in the batch.
    pub(crate) async fn check_budget(&self, db: &mut SqliteConnection, user: &str, amount: i64, unknown: bool, now: i64) -> Result<()> {
        let caps: Option<(Option<i64>, Option<i64>)> =
            sqlx::query_as("SELECT daily_micros, monthly_micros FROM budgets WHERE user_id = ?").bind(user).fetch_optional(&mut *db).await?;
        let (daily, monthly) = caps.unwrap_or_default();
        if daily.is_none() && monthly.is_none() {
            return Ok(());
        }
        if unknown {
            return Err(EngineError::invalid(
                "This paid model has no cost estimate. Choose a priced model or remove the budget cap before running it.",
                Some("budget"),
            ));
        }
        let (day, month) = boundaries(db, now).await?;
        for (name, cap, since) in [("daily", daily, day), ("monthly", monthly, month)] {
            if let Some(cap) = cap {
                let spent = total(db, user, since).await?;
                let left = (cap - spent).max(0);
                if amount > left {
                    return Err(EngineError::invalid(
                        format!(
                            "The {name} budget cap is ${:.6}. ${:.6} is left; this run needs ${:.6}.",
                            cap as f64 / 1e6,
                            left as f64 / 1e6,
                            amount as f64 / 1e6
                        ),
                        Some("budget"),
                    ));
                }
            }
        }
        Ok(())
    }

    pub async fn spend_view(&self, user: &str) -> Result<SpendView> {
        // One snapshot keeps totals, breakdowns and remaining caps consistent.
        let mut tx = self.inner.db.begin().await?;
        let (day, month) = boundaries(&mut tx, now_ms()).await?;
        let since = day - 29 * DAY;
        let today = total(&mut tx, user, day).await?;
        let week = total(&mut tx, user, day - 6 * DAY).await?;
        let thirty = total(&mut tx, user, since).await?;
        let month_spent = total(&mut tx, user, month).await?;
        let caps: Option<(Option<i64>, Option<i64>)> =
            sqlx::query_as("SELECT daily_micros, monthly_micros FROM budgets WHERE user_id = ?").bind(user).fetch_optional(&mut *tx).await?;
        let (daily, monthly) = caps.unwrap_or_default();
        let days = sqlx::query_as(
            "WITH RECURSIVE days(at) AS (SELECT ? UNION ALL SELECT at + 86400000 FROM days WHERE at < ?)
            SELECT strftime('%Y-%m-%d', days.at / 1000, 'unixepoch') AS name,
                COALESCE(sum(COALESCE(s.actual_micros, s.estimated_micros, 0)), 0) / 1000000.0 AS usd
            FROM days LEFT JOIN spend_runs s ON s.user_id = ? AND s.created_at >= days.at AND s.created_at < days.at + 86400000
            GROUP BY days.at ORDER BY days.at",
        )
        .bind(since)
        .bind(day)
        .bind(user)
        .fetch_all(&mut *tx)
        .await?;
        let mut groups = Vec::new();
        for field in ["model", "provider"] {
            groups.push(sqlx::query_as::<_, SpendGroup>(&format!("SELECT {field} AS name, sum(COALESCE(actual_micros, estimated_micros, 0)) / 1000000.0 AS usd FROM spend_runs WHERE user_id = ? AND created_at >= ? GROUP BY {field} ORDER BY usd DESC, name"))
                .bind(user).bind(since).fetch_all(&mut *tx).await?);
        }
        let (unknown_runs, estimated_runs): (i64, i64) = sqlx::query_as("SELECT COALESCE(sum(actual_micros IS NULL AND estimated_micros IS NULL), 0), COALESCE(sum(actual_micros IS NULL AND estimated_micros > 0), 0) FROM spend_runs WHERE user_id = ? AND created_at >= ?")
            .bind(user).bind(since).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        let providers = groups.pop().unwrap();
        let models = groups.pop().unwrap();
        Ok(SpendView {
            today_usd: today as f64 / 1e6,
            seven_days_usd: week as f64 / 1e6,
            thirty_days_usd: thirty as f64 / 1e6,
            days,
            models,
            providers,
            daily: progress(daily, today),
            monthly: progress(monthly, month_spent),
            unknown_runs,
            estimated_runs,
            timezone: "UTC",
        })
    }
}
