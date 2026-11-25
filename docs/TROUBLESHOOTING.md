# Troubleshooting

Open the settings folder from the tray menu (**Open settings folder**) to reach the files below.

| What you see | Why | What to do |
|---|---|---|
| No tray icon | Windows 11 hides new icons | Click the `^` arrow in the notification area and drag the icon out |
| **Not logged in** / sign-in alert | Claude Code is not signed in, or the login expired | Run `claude` in a terminal and sign in again. The app recovers by itself within one refresh |
| **Claude Code CLI not found** | `claude` is not on PATH for this user | Settings > Claude Code > path, e.g. `C:\Users\you\.local\bin\claude.exe` |
| **Not available: …signed in with a claude.ai account** | You use an API key, which has no subscription limits | Sign in with a claude.ai account to see limits |
| **Error: could not parse provider output** or a ⚠ "unrecognised output" row | A Claude Code update changed the wording of `/usage` | Update Agent Usage Monitor. Until then the values that still parse are shown and nothing is guessed |
| Numbers marked **Stale** | The last refresh failed (offline, CLI error). Old values are kept on purpose | Check your network, press Refresh. Stale means older than two refresh intervals |
| "reset passed, refreshing" | A limit window rolled over since the last read | Waits for the next refresh; press Refresh to speed it up |
| Percentages differ from the Claude website by a point | Claude Code prints whole numbers; the website may round differently | Expected |
| No toast notifications | Focus Assist / Do Not Disturb, or notifications off for the app | Windows Settings > System > Notifications. Use Settings > Notifications > **Send a test notification**. Check `diagnostics.log` for `toast failed` |
| Toasts say "Windows PowerShell" | Portable mode has no registered app name | Use the installer, or ignore it |
| **Launch at login** does nothing | Disabled in Task Manager > Startup apps, or the exe moved | Re-enable it there; or toggle the setting off and on in Settings |
| Widget is off-screen | A monitor was unplugged | It returns to the main monitor on the next start. Or delete `widget_pos` in `settings.json` |
| Widget ignores the mouse | **Click-through** is on | Tray menu > Widget > uncheck **Click-through** |
| Widget hidden and you want it back | You closed it with × or Esc | Tray menu > **Show widget** |
| Windows says it blocked the app (SmartScreen) | Builds are not code-signed yet | **More info > Run anyway**; compare `SHA256SUMS.txt` |
| The app will not start | A damaged settings file is handled automatically; otherwise see `startup-error.txt` | Delete `settings.json` (it is recreated), then check `startup-error.txt` |

## Session activity

| What you see | Why | What to do |
|---|---|---|
| "Sessions: none running" but Claude is open | The session is in WSL (the desktop app runs them there) and the WSL bridge is off | Settings > Session activity > WSL distributions: enter the distribution (for example `Ubuntu`; `wsl -l -q` lists them) |
| "activity unavailable" | `claude agents --json` failed or its format changed | Update Claude Code and this app; run `claude agents --json` yourself to compare |
| "this Claude Code version has no `claude agents`" | An old Claude Code | Update Claude Code |
| WSL sessions missing though a distribution is named | The distribution is **stopped** (it is never started) or `claude` is somewhere unusual | Start it (open a WSL terminal), or set the path to `claude` inside WSL in Settings |
| "finished" never appears | The turn was shorter than the poll interval (15 s while busy, 60 s when quiet), or the session was already idle when the app started | Expected: finish is inferred between two polls |
| A count that includes old idle sessions | Idle terminals count as live sessions | They are hidden whenever something is working or waiting |
| Needs-input alert never fires | The alert switch is off (default) or the session was already waiting when the app started | Settings > Session activity |

## Codex CLI

| What you see | Why | What to do |
|---|---|---|
| **Codex CLI not found** | `codex` is not on PATH for this app | Settings > Codex CLI > path (Windows npm installs are `%APPDATA%\npm\codex.cmd`) |
| **Not logged in** for Codex | Codex is not signed in | Run `codex` and sign in with ChatGPT |
| **Not available: …ChatGPT account** | Codex uses an API key | Sign in with ChatGPT to get subscription limits |
| **Error: unexpected rate-limit response** | A Codex update changed `app-server` | Update Agent Usage Monitor; Codex's `app-server` is labelled experimental |
| "no rate-limit windows reported" | Codex answered with no limits (it can do that right after starting) | The app already retries once; press Refresh |

## macOS

| What you see | Why | What to do |
|---|---|---|
| "cannot be opened because the developer cannot be verified" | The app is not notarized | Right-click the app > Open > Open (once) |
| No menu bar item | Hidden behind the notch or by a menu bar manager | Make room, or use a tool that shows overflow items |
| **CLI not found** though `claude` works in Terminal | Finder-launched apps do not get your shell `PATH` | Set the path in Settings (`which claude` shows it) |
| Keychain prompt on first refresh | Claude Code reads its own login from the Keychain | Choose **Always Allow** for `claude` |
| Notifications say "Script Editor" | Notifications are sent through `osascript` | Expected; allow them in System Settings > Notifications |
| Widget corners are square | No rounded-corner call on macOS yet | Known limitation |

## Update check

- **"the repository's releases are not publicly visible (HTTP 404)"**: the project's GitHub repository is
  private (or renamed). The check can only read public releases.
- **"GitHub is limiting requests from this network"**: unauthenticated GitHub requests are limited per IP
  address (60 an hour, shared by everyone behind one router). It tries again within a day; wait and retry.
- **"curl is not available"**: the check uses the system's `curl` (Windows 10 version 1803 and later, macOS).
  Without it the check is unavailable; look for new versions on the releases page instead.
- **No question appeared / want it again**: the question is asked once. Use Settings > Updates, or the tray
  menu's "Check for updates…". To be asked again, quit the app and remove `"prompted"` from the `update`
  section of `settings.json`.
- Every failure is written to `diagnostics.log` ("update check failed: …"); `update.json` holds the last one.

## Starting over

Quit from the tray menu, delete `%APPDATA%\AgentUsageMonitor`, start the app again.

## Checking what it does

The app only runs two commands, never through a shell:

```powershell
claude auth status
claude -p "/usage" --no-session-persistence --output-format text
```

Run them yourself to compare. If `claude -p "/usage"` shows no limits, neither will the app.

## Reporting a problem

Open an issue with: Windows version, Claude Code version (`claude --version`), what you saw, and the
last lines of `diagnostics.log`. Do not paste `auth status` output; it contains your email.
