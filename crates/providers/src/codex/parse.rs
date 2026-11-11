//! Parsers for the Codex `app-server` responses we use.
//!
//! Source of truth: the JSON schemas published in `openai/codex`
//! (`GetAccountRateLimitsResponse`, `GetAccountResponse`). Unknown fields are ignored so a newer
//! Codex that adds fields keeps working; a missing required field is an error, never a guess.
//!
//! Windows are identified by their **duration**, not by whether they are `primary` or `secondary`:
//! 300 minutes is the short rolling window and 10080 minutes is the weekly window.

use std::collections::HashMap;

use aum_core::{Provenance, ProviderError, UsageWindow, WindowKind};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Minutes in the two windows Codex reports today.
const SHORT_MINS: i64 = 300;
const WEEK_MINS: i64 = 10_080;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitsResponse {
    rate_limits: Snapshot,
    #[serde(default)]
    rate_limits_by_limit_id: Option<HashMap<String, Snapshot>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    limit_id: Option<String>,
    limit_name: Option<String>,
    plan_type: Option<String>,
    primary: Option<Window>,
    secondary: Option<Window>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Window {
    used_percent: i64,
    window_duration_mins: Option<i64>,
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub windows: Vec<UsageWindow>,
    pub plan: Option<String>,
    /// Things we could not use (a window with no reset time, a duplicate window).
    pub unrecognized: Vec<String>,
}

/// Which window is this, by length? `position_fallback` is used only when the duration is absent.
fn classify(mins: Option<i64>, position_fallback: WindowKind) -> WindowKind {
    match mins {
        Some(m) if (m - SHORT_MINS).abs() <= 15 => WindowKind::Session,
        Some(m) if (m - WEEK_MINS).abs() <= 120 => WindowKind::Weekly,
        Some(m) if m > 0 && m % 1440 == 0 => WindowKind::Named(format!("{}-day", m / 1440)),
        Some(m) if m > 0 && m % 60 == 0 => WindowKind::Named(format!("{}-hour", m / 60)),
        Some(m) if m > 0 => WindowKind::Named(format!("{m}-minute")),
        _ => position_fallback,
    }
}

/// Rename a kind so it is clearly the extra bucket's: "Spark 5-hour", "Weekly (Spark)".
fn for_bucket(kind: WindowKind, bucket: &str) -> WindowKind {
    match kind {
        WindowKind::Session => WindowKind::Named(format!("{bucket} 5-hour")),
        WindowKind::Weekly => WindowKind::WeeklyModel(bucket.to_owned()),
        WindowKind::Named(n) => WindowKind::Named(format!("{bucket} {n}")),
        other => other,
    }
}

fn to_window(w: &Window, kind: WindowKind, unrecognized: &mut Vec<String>) -> Option<UsageWindow> {
    let Some(secs) = w.resets_at else {
        unrecognized.push(format!("{} window has no reset time", kind.label()));
        return None;
    };
    let Some(resets_at) = DateTime::<Utc>::from_timestamp(secs, 0) else {
        unrecognized.push(format!("{} window has an invalid reset time", kind.label()));
        return None;
    };
    let percent = w.used_percent.clamp(0, 100) as u32;
    Some(UsageWindow::new(
        kind,
        percent,
        resets_at,
        Provenance::Official,
    ))
}

fn push_unique(out: &mut Vec<UsageWindow>, unrecognized: &mut Vec<String>, w: UsageWindow) {
    if out.iter().any(|x| x.kind == w.kind) {
        unrecognized.push(format!("duplicate {} window ignored", w.kind.label()));
    } else {
        out.push(w);
    }
}

/// Parse the `result` of `account/rateLimits/read`.
pub fn parse_rate_limits(result: &serde_json::Value) -> Result<Parsed, ProviderError> {
    let r: RateLimitsResponse = serde_json::from_value(result.clone())
        .map_err(|e| ProviderError::Parse(format!("unexpected rate-limit response: {e}")))?;

    let mut windows = Vec::new();
    let mut unrecognized = Vec::new();

    for (w, fallback) in [
        (&r.rate_limits.primary, WindowKind::Session),
        (&r.rate_limits.secondary, WindowKind::Weekly),
    ] {
        if let Some(w) = w {
            let kind = classify(w.window_duration_mins, fallback);
            if let Some(win) = to_window(w, kind, &mut unrecognized) {
                push_unique(&mut windows, &mut unrecognized, win);
            }
        }
    }

    // Additional metered buckets (for example a separate model family), sorted for stable output.
    if let Some(buckets) = &r.rate_limits_by_limit_id {
        let main_id = r.rate_limits.limit_id.as_deref();
        let mut extra: Vec<(&String, &Snapshot)> = buckets
            .iter()
            .filter(|(id, snap)| {
                Some(id.as_str()) != main_id && snap.limit_id.as_deref() != main_id
            })
            .collect();
        extra.sort_by_key(|(id, _)| id.as_str());
        for (id, snap) in extra {
            let name = snap.limit_name.clone().unwrap_or_else(|| id.clone());
            for (w, fallback) in [
                (&snap.primary, WindowKind::Session),
                (&snap.secondary, WindowKind::Weekly),
            ] {
                if let Some(w) = w {
                    let kind = for_bucket(classify(w.window_duration_mins, fallback), &name);
                    if let Some(win) = to_window(w, kind, &mut unrecognized) {
                        push_unique(&mut windows, &mut unrecognized, win);
                    }
                }
            }
        }
    }

    if windows.is_empty() {
        return Err(ProviderError::Parse(if unrecognized.is_empty() {
            "no rate-limit windows reported".to_owned()
        } else {
            unrecognized.join("; ")
        }));
    }
    Ok(Parsed {
        windows,
        plan: r.rate_limits.plan_type,
        unrecognized,
    })
}

/// What `account/read` says about who is signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    ChatGpt {
        plan: Option<String>,
    },
    ApiKey,
    /// Some other way of authenticating that has no ChatGPT subscription limits.
    Other(String),
    SignedOut,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountResponse {
    account: Option<serde_json::Value>,
}

/// Parse the `result` of `account/read`. Only the fields needed are read: the account type and
/// plan. The email address is present in the response but deliberately never read.
pub fn parse_account(result: &serde_json::Value) -> Result<Account, ProviderError> {
    let r: AccountResponse = serde_json::from_value(result.clone())
        .map_err(|e| ProviderError::Parse(format!("unexpected account response: {e}")))?;
    let Some(account) = r.account else {
        return Ok(Account::SignedOut);
    };
    match account.get("type").and_then(|t| t.as_str()) {
        Some("chatgpt") => Ok(Account::ChatGpt {
            plan: account
                .get("planType")
                .and_then(|p| p.as_str())
                .map(str::to_owned),
        }),
        Some("apiKey") => Ok(Account::ApiKey),
        Some(other) => Ok(Account::Other(other.to_owned())),
        None => Err(ProviderError::Parse("account has no type".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixture(name: &str) -> serde_json::Value {
        let path = format!(
            "{}/../../tests/fixtures/codex/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(
            &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")),
        )
        .unwrap()
    }

    #[test]
    fn normal_response_gives_session_and_weekly() {
        let p = parse_rate_limits(&fixture("rate_limits_normal.json")).unwrap();
        assert_eq!(
            p.windows.len(),
            2,
            "the main bucket is not repeated from rateLimitsByLimitId"
        );
        assert_eq!(p.windows[0].kind, WindowKind::Session);
        assert_eq!(p.windows[0].used_percent, 25);
        assert_eq!(
            p.windows[0].resets_at,
            Utc.timestamp_opt(1_779_459_394, 0).unwrap()
        );
        assert_eq!(p.windows[1].kind, WindowKind::Weekly);
        assert_eq!(p.windows[1].used_percent, 18);
        assert_eq!(p.plan.as_deref(), Some("plus"));
        assert!(p.unrecognized.is_empty());
        assert!(
            p.windows
                .iter()
                .all(|w| w.provenance == Provenance::Official)
        );
    }

    #[test]
    fn extra_metered_bucket_is_labelled_separately() {
        let p = parse_rate_limits(&fixture("rate_limits_extra_bucket.json")).unwrap();
        let labels: Vec<String> = p.windows.iter().map(|w| w.kind.label()).collect();
        assert_eq!(
            labels,
            vec!["Session", "Weekly", "Spark 5-hour", "Weekly (Spark)"]
        );
        assert_eq!(p.windows[3].used_percent, 92);
    }

    #[test]
    fn windows_are_identified_by_duration_not_position() {
        // A weekly window reported as `primary` must still be the weekly window.
        let v = serde_json::json!({"rateLimits":{
            "primary":{"usedPercent":40,"windowDurationMins":10080,"resetsAt":1779826837},
            "secondary":{"usedPercent":7,"windowDurationMins":300,"resetsAt":1779459394}}});
        let p = parse_rate_limits(&v).unwrap();
        assert_eq!(p.windows[0].kind, WindowKind::Weekly);
        assert_eq!(p.windows[1].kind, WindowKind::Session);
    }

    #[test]
    fn unusual_and_missing_durations() {
        let p = parse_rate_limits(&fixture("rate_limits_unusual.json")).unwrap();
        assert_eq!(p.windows[0].kind, WindowKind::Named("1-hour".into()));
        // No duration at all: fall back to the position (secondary = weekly).
        assert_eq!(p.windows[1].kind, WindowKind::Weekly);
    }

    #[test]
    fn window_without_reset_time_is_skipped_and_reported() {
        let p = parse_rate_limits(&fixture("rate_limits_missing_reset.json")).unwrap();
        assert_eq!(p.windows.len(), 1);
        assert_eq!(p.windows[0].kind, WindowKind::Weekly);
        assert_eq!(p.unrecognized, vec!["Session window has no reset time"]);
    }

    #[test]
    fn empty_response_is_an_error_not_zero_percent() {
        let e = parse_rate_limits(&fixture("rate_limits_empty.json")).unwrap_err();
        assert!(matches!(e, ProviderError::Parse(m) if m.contains("no rate-limit windows")));
    }

    #[test]
    fn missing_required_fields_are_errors() {
        assert!(parse_rate_limits(&serde_json::json!({})).is_err());
        assert!(parse_rate_limits(&serde_json::json!({"rateLimits":{"primary":{"windowDurationMins":300,"resetsAt":1}}})).is_err());
        assert!(parse_rate_limits(&serde_json::json!("nope")).is_err());
    }

    #[test]
    fn percent_is_clamped_and_unknown_fields_ignored() {
        let v = serde_json::json!({"rateLimits":{"futureField":{"x":1},
            "primary":{"usedPercent":140,"windowDurationMins":300,"resetsAt":1779459394,"extra":true}},
            "somethingNew":[1,2,3]});
        assert_eq!(parse_rate_limits(&v).unwrap().windows[0].used_percent, 100);
        let neg = serde_json::json!({"rateLimits":{"primary":{"usedPercent":-3,"windowDurationMins":300,"resetsAt":1779459394}}});
        assert_eq!(parse_rate_limits(&neg).unwrap().windows[0].used_percent, 0);
    }

    #[test]
    fn duplicate_windows_keep_the_first() {
        let v = serde_json::json!({"rateLimits":{
            "primary":{"usedPercent":10,"windowDurationMins":300,"resetsAt":1779459394},
            "secondary":{"usedPercent":90,"windowDurationMins":300,"resetsAt":1779459999}}});
        let p = parse_rate_limits(&v).unwrap();
        assert_eq!(p.windows.len(), 1);
        assert_eq!(p.windows[0].used_percent, 10);
        assert_eq!(p.unrecognized.len(), 1);
    }

    #[test]
    fn accounts() {
        assert_eq!(
            parse_account(&fixture("account_chatgpt.json")).unwrap(),
            Account::ChatGpt {
                plan: Some("plus".into())
            }
        );
        assert_eq!(
            parse_account(&fixture("account_apikey.json")).unwrap(),
            Account::ApiKey
        );
        assert_eq!(
            parse_account(&fixture("account_signed_out.json")).unwrap(),
            Account::SignedOut
        );
        let other = serde_json::json!({"account":{"type":"gcp"},"requiresOpenaiAuth":false});
        assert_eq!(parse_account(&other).unwrap(), Account::Other("gcp".into()));
        assert!(parse_account(&serde_json::json!({"account":{"email":"x"}})).is_err());
        assert!(parse_account(&serde_json::json!(5)).is_err());
    }
}
