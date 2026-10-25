//! Pictesa core: the shared vocabulary between the engine, the server and every provider.
//!
//! A *provider* is a backend that can run generations (fal, OpenAI, a local ComfyUI, ...).
//! A *model* is one thing a provider can run, described by [`ModelSpec`]: which media it
//! produces, which media it accepts and which parameters it exposes. The UI, the CLI and the
//! MCP tools are all generated from these specs, so adding a model never needs UI code.

pub mod error;
pub mod model;
pub mod lora;
pub mod provider;
pub mod request;
pub mod schema;

pub use error::ProviderError;
pub use model::*;
pub use provider::*;
pub use request::*;

/// The UTC calendar date (YYYY-MM-DD) of a Unix time in seconds.
pub fn utc_date(secs: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Splits a full model id `provider:model` into its two parts.
pub fn split_model_id(full: &str) -> Option<(&str, &str)> {
    let (provider, model) = full.split_once(':')?;
    (!provider.is_empty() && !model.is_empty()).then_some((provider, model))
}

#[cfg(test)]
mod tests {
    #[test]
    fn utc_dates_follow_the_calendar() {
        assert_eq!(super::utc_date(0), "1970-01-01");
        assert_eq!(super::utc_date(-1), "1969-12-31");
        assert_eq!(super::utc_date(951_868_740), "2000-02-29");
        assert_eq!(super::utc_date(1_792_886_400), "2026-10-25");
    }
}
