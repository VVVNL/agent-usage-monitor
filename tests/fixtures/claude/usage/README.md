# Claude `/usage` fixtures

Captured 2026-11-10 from Claude Code 2.1.322, Team seat, via `claude -p "/usage"`.
Personal data (email, org, ids, paths, MCP server name) is redacted.

| File | State | Notes |
|---|---|---|
| text_normal_28pct.txt | normal | New 5h window after a reset; model-weekly (Fable) line present |
| text_high_82pct.txt | high usage | Hand-copied from an earlier same-day run; reset time was before the 28% capture, so also shows window rollover |
| text_logged_out.txt | no login | Run with an empty CLAUDE_CONFIG_DIR. Exit code 0, no limit lines, only a cost summary |
| text_mac_at_format.txt | newer wording | Captured 2026-11-26 on a Mac (real Claude Code, version not recorded). Reset times read `Nov 26 at 12:19am`: no comma, with `at`. No model-weekly line |
| json_normal.json | `--output-format json` | Limit text sits in `.result`; usage/cost fields all zero |
| auth_status_team.json | `claude auth status` | Structured JSON: loggedIn, subscriptionType, loginExpiresAt |

Not yet captured (to be added as clearly marked synthetic fixtures in Phase 1):
100% / exhausted, no model-weekly line, multiple model-weekly lines, expired login,
non-English or changed wording, non-London timezone, year rollover.
