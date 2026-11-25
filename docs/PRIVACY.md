# Privacy and security

Agent Usage Monitor is local-first and read-only.

**What it runs.** Started directly with an argument list (no shell):

- Claude Code: `claude auth status` and
  `claude -p "/usage" --no-session-persistence --output-format text`.
- Codex CLI (only if you turn it on): `codex app-server`, then exactly four messages: `initialize`,
  `initialized`, `account/read` and `account/rateLimits/read`. The process is stopped, with everything
  it started, after every poll.
- Session activity (Claude Code): `claude agents --json`. On Windows, if you name a WSL distribution:
  `wsl.exe -l --running -q` (lists running distributions; never starts one) and, for each configured
  running distribution, `wsl.exe -d <name> --exec sh -c <fixed script> sh <path>` where the script is a
  constant that only looks for `claude` and runs `claude agents --json`. Nothing you type is
  interpolated into it.
- macOS only: `osascript` to show a notification, and `open` to open the settings folder.

**What it reads.** The limit lines of `/usage` and, from `auth status`, only `loggedIn`,
`authMethod`, `subscriptionType` and `loginExpiresAt`. From Codex: the account type and plan, and the
rate-limit windows (percent used, window length, reset time). Codex's response also contains your email
address; it is never read into the program. Your email, organisation and paths are present in
that output but are deliberately not read into the program, so they can never be stored or logged.

From `claude agents --json` it reads only `pid`, `startedAt`, `status`, `state` and `waitingFor`. The same
output contains the project folder (`cwd`), the conversation title (`name`) and the session id; those
fields are dropped by the parser and never stored, shown or logged. Session activity is counts and
short states only.

**What it never does.**

- It never opens Claude Code's credential files, keychain entries or tokens.
- It makes no network connections of its own, with one optional exception (the update check below):
  there is no HTTP client in the program. Claude Code does
  the talking, exactly as when you run `/usage` yourself.
- It does not modify any Claude Code file or setting.
- It does not read conversations, transcripts or project files.
- It has no telemetry, analytics or crash reporting, and never downloads or installs anything.
- `--no-session-persistence` stops each refresh from writing a transcript file into `~/.claude`.
- Running `/usage` makes no model call (zero tokens, zero cost), so refreshing does not use quota.

**What it stores** (all under `%APPDATA%\AgentUsageMonitor\config`): preferences, the last known
limit percentages and reset times, the list of alerts already shown, and a small diagnostics log of its
own status messages. No credentials, no account identifiers.

**Startup entry.** If you enable *Launch at login* it writes one per-user entry: on Windows the value
`HKCU\...\Run\AgentUsageMonitor`, on macOS the file
`~/Library/LaunchAgents/io.github.agent-usage-monitor.plist`. Neither needs administrator rights.

**Supply chain.** Dependencies are pinned in `Cargo.lock`, built in CI from source, and listed in
[THIRD_PARTY.md](THIRD_PARTY.md). Release files come with SHA-256 checksums. Builds are not yet
code-signed.

**Reporting a vulnerability.** Please open a private security advisory on the repository rather than a
public issue.

## Update check (optional)

The only network request the app itself can make. **Off until you say yes**: about 20 seconds after the
first start the app asks once ("Check for new versions?"); closing that window counts as no. You can change
it later in Settings > Updates, and the tray menu's "Check for updates…" sends one request when you click it,
even with the daily check off.

- **What is sent:** one HTTPS request to `api.github.com` (the list of this project's releases), made by the
  operating system's `curl`, with a generic `User-Agent: agent-usage-monitor`. No version, no account, no
  usage, no machine identifier. GitHub necessarily sees your IP address, as with any web request. It carries
  no credentials.
- **What comes back** is read for each release's tag name and whether it is a pre-release or draft, nothing
  else. Anything unexpected is rejected.
- **What is stored:** `update.json` in the settings folder (time of the last check, an ETag so the next
  question can be answered "unchanged", the tags seen, the last error). Delete it any time.
- **What it does with a newer version:** shows "Update available: X" in the tray menu and one notification.
  Choosing it opens the release page in your browser. **Nothing is downloaded, installed or run by the app.**
- Frequency: at most once every 24 hours, plus your own clicks (at most one request a minute).
