# Codex `app-server` fixtures

**Schema-derived, not captured from a running Codex.** They follow the published JSON schema in
`openai/codex` (`codex-rs/app-server-protocol/schema/json/v2/GetAccountRateLimitsResponse.json` and
`GetAccountResponse.json`) and an example response reported for Codex 0.133.0 in a public issue
(`rateLimits.primary` 300 minutes, `secondary` 10080 minutes). Values and reset times are invented.

Replace or add real captures once a Codex CLI has been run against this machine's account, and
re-run `cargo test -p aum-providers` to confirm the parser still agrees.

| File | Shape |
|---|---|
| rate_limits_normal.json | primary 5-hour + secondary weekly, one bucket |
| rate_limits_extra_bucket.json | main bucket plus a second metered bucket in `rateLimitsByLimitId` |
| rate_limits_unusual.json | durations that are neither 5 h nor weekly, and a missing duration |
| rate_limits_missing_reset.json | a window without `resetsAt` |
| rate_limits_empty.json | `primary` and `secondary` both null (seen when asked too early) |
| account_chatgpt.json | `account.type` = `chatgpt` with `planType` |
| account_apikey.json | `account.type` = `apiKey` |
| account_signed_out.json | `account` null, `requiresOpenaiAuth` true |
