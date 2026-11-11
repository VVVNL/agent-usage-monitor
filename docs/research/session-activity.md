# Study: showing active and recently finished sessions in the widget

Status: **built in 0.11.0** (phases 1, 2 and 4: Claude via `claude agents --json`, alerts, WSL
bridge). Not built: the optional session-file fast path (phase 3), Codex hooks (5) and the Codex
database fallback (6). See the README for what was verified. Written 2026-11-11.

**Question.** Can the widget show, per provider, whether sessions are running right now, whether one
needs the user's attention, and which finished recently?

**Short answer.** Yes for Claude Code, through an official interface. For Codex, only through hooks
the user installs, or an internal database we would be reading without a support promise.

Everything marked *observed* was checked on this machine. Everything marked *docs* comes from the
providers' documentation. Nothing about Codex was run against a live Codex.

## What we want to show

| State | Meaning | Example text |
|---|---|---|
| Working | a turn is running | `● 2 working` |
| Needs input | blocked on a permission prompt, question or dialog | `⚠ 1 needs input (permission prompt)` |
| Finished | a turn finished recently | `✓ finished 4 min ago` |
| Idle | sessions exist, nothing to report | `○ 1 idle` |
| none | no live session | (row hidden) |

Counts only by default. Project folders and session names are opt-in (see Privacy).

## Claude Code

### Sources, best first

| # | Source | Supported? | Gives | Cost | Verdict |
|---|---|---|---|---|---|
| A | `claude agents --json` | **Yes.** Docs call it "the supported way to read session state from outside Claude Code" | per live process: `pid`, `kind` (`interactive`/`background`), `startedAt`, `status` = `busy`/`waiting`/`idle`, and `waitingFor` (`permission prompt`, `input needed`, `sandbox request`, `worker request`, `dialog open`); background sessions also `state` = `working`/`blocked`/`done`/`failed`/`stopped` | *observed:* 0.2 s wall, 0.14 s CPU, 136 MB transient memory per call; no transcript or session file is written; our own call is not listed | **Primary.** Poll adaptively |
| B | `~/.claude/sessions/<pid>.json` | **No** (not in the docs) | same status, plus `statusUpdatedAt` (ms) = exact time of the last transition | *observed:* 0.38 ms for 43 files; can be watched with OS file events (zero idle cost) | Optional fast path. Needs a liveness check: 41 of 43 files here belong to dead processes and still say `idle` |
| C | Hooks (`Stop`, `Notification`, `SessionEnd`, `SubagentStop`, `PermissionRequest`...) | **Yes** | exact, event-driven transitions | a tiny helper process per event | Best fidelity, but the user must edit `settings.json`; we only print a snippet |
| D | Transcript file modification times | n/a | "something wrote in the last N seconds" | stat only | Weak fallback. Cannot tell waiting from working |
| E | Process list | n/a | a `claude` process exists | cheap | Cannot tell busy from idle. Rejected |

### Observed facts (this machine, 2026-11-10)

- 43 session files: 2 live (1 `busy`, 1 `idle`), 41 dead. Entry points seen: `claude-desktop` (40), `cli`,
  `sdk-cli`. All `kind: interactive`. Timestamps are in milliseconds.
- `claude agents --json` printed exactly the 2 live sessions, with keys `pid`, `cwd`, `kind`,
  `startedAt`, `sessionId`, `name`, `status`.
- `pidDomain` in the session files names a Linux PID namespace. **Sessions started by the Claude
  desktop app on this machine run inside WSL**, so a Windows program cannot test their PIDs directly.

### Limits to design around

- **Research preview.** Docs: "the interface and keyboard shortcuts may change", and no schema
  stability promise. Unknown `status` values must become "unknown", never "idle".
- **No "since when" in source A.** "Finished 4 min ago" must be inferred by watching `busy` become
  `idle`. Granularity is the poll interval; a turn shorter than the interval can be missed entirely.
  Source B (`statusUpdatedAt`) fixes both, at the price of using an undocumented file.
- **A session that disappears** (terminal closed) cannot be told apart from one that ended normally.
  Show it as "ended", not "finished".
- **Subagents are not listed** separately (docs), so a parent that is waiting on subagents reads as
  `busy`.
- `waitingFor` is the valuable signal: it is the only way to say "Claude is waiting for you" without
  hooks.

## Codex CLI

### Sources

| # | Source | Supported? | Gives | Verdict |
|---|---|---|---|---|
| A | Any listing command / API | **No.** The config docs have none | - | Does not exist |
| B | Hooks in `~/.codex/hooks.json` or `config.toml` (`SessionStart`, `UserPromptSubmit`, `PermissionRequest`, `Stop`, `Interrupt`, `SessionEnd`, ...) | **Yes**, enabled by default (docs) | `session_id`, `turn_id`, `cwd`, `hook_event_name` per event | **Primary for Codex.** User action required; Windows behaviour not documented |
| C | `notify` in `config.toml` | **Yes** | only `agent-turn-complete` (+ `thread-id`, `turn-id`, `cwd`) | Finished only; no working/waiting |
| D | `~/.codex/thread_history_1.sqlite` table `thread_turns` (`status`, `started_at`, `completed_at`); `~/.codex/state_5.sqlite` table `threads` | **No.** Internal, schema-versioned by file name (`state_5`) | precise turn state | Possible, read-only, behind a schema guard. Fragile |
| E | `codex app-server` events | Yes | events for threads **inside that server process only** | Useless: our own server sees none of the user's other sessions |

### Observed facts

- This machine's Codex data is old: 1 thread, 1 turn with `status = completed`, timestamps in
  **seconds** (unlike Claude's milliseconds). So the in-progress representation (probably a null
  `completed_at`) is **unverified**, and a crash would leave a turn looking in progress forever
  (needs a staleness cutoff).
- No Codex binary is installed, so nothing could be run.
- The same databases hold conversation text (`item_json`, `title`, `raw_memory`). A reader must select
  named columns only, never `*`.

## Privacy

The project rule is "never collect conversation content". Session state touches it in three places:

1. **Session files and `agents --json` carry `cwd` and `name`** (project folder and conversation
   title). Deserialize only `pid`, `kind`, `status`, `waitingFor`, `startedAt`, `statusUpdatedAt`; serde
   drops the rest. Never log or store the dropped fields.
2. **Hook payloads contain content by design** (Codex: `prompt`, `last_assistant_message`; Claude:
   prompts and notification text). A hook helper necessarily receives it on stdin. It must keep only
   event name, session id hash and timestamp, and discard the rest in memory. This is the strongest
   argument for preferring source A (no content is ever handed to us) and for making hooks opt-in with
   an explicit disclosure.
3. **A widget that shows project names is a screenshot risk.** Default off.

## How it fits the architecture

Activity is a separate concept from quota, so it gets its own type and its own schedule:

```
core        SessionActivity { key, state: Working | NeedsInput(reason) | Idle | Unknown, since: Option<time> }
            ActivitySnapshot { provider, sessions, observed_at }
            trait ActivitySource                                   (parallel to Provider)
providers   claude::agents  (parse `claude agents --json`, fail closed)
            claude::session_files (optional), codex::hooks / codex::db (later)
collector   second per-provider task, adaptive interval            (isolated like quota tasks)
app         ActivityTracker (pure): snapshots -> {working, needs-input, recently finished with TTL}
            view/widget gain an activity line; notify gains two kinds
```

- **Polling budget.** `agents --json` costs about 0.14 s CPU per call. Every 60 s with no live
  session and every 15 s while something is working is about 0.2 to 1% of one core, in short bursts. A
  file watcher on `~/.claude/sessions` (source B) would make it event-driven with no idle cost.
- **The tracker is pure** (like `notify`), so transition logic (busy to idle, missed short turns,
  disappeared sessions, unknown statuses, pid reuse via `startedAt`) is unit-tested on any OS.
- **Notifications** reuse the existing engine: "needs input" (once per wait, with `waitingFor`) and
  "finished" (only if the turn ran longer than N seconds, with a cooldown). Both off by default.

## Platform notes

- **macOS and Linux:** same commands, no namespace problem.
- **Windows with native `claude.exe`:** `claude.exe agents --json` works the same way.
- **Windows with sessions in WSL (this machine's case):** needs `wsl.exe -d <distro> -e claude agents
  --json`. Starting `wsl.exe` can boot a stopped distro (seconds and hundreds of MB), so check
  `wsl --list --running` first and never wake a stopped one. Reading `\\wsl.localhost\...` wakes it too.
  Treat WSL as an explicit, per-distro opt-in setting.
- **Where Codex runs** has the same split.

### WSL bridge: measured on this machine (2026-11-11)

| Call | Time |
|---|---|
| `wsl -l --running` (lists running distros, never starts one) | 42 ms |
| `wsl -d Ubuntu -e claude agents --json` (VM already running) | 277 to 307 ms |
| Windows-native `claude.exe agents --json` | 244 ms |

- The bridge returned the same 7 fields as the native call, for the 2 live WSL sessions.
- **Two independent session stores exist.** Windows has `%USERPROFILE%\.claude\sessions` (1 file); WSL
  has its own (43 files, 40 of them from the desktop app). A Windows program that skips the bridge sees
  almost none of this user's sessions.
- **A stopped distro cannot have running sessions** (they live inside it), so "only query distros that
  `wsl -l --running` lists" loses no information and can never wake the VM.
- Unverified: whether polling every 15 s keeps WSL's idle shutdown from ever firing (a distro normally
  stops a few seconds after its last process exits). Needs a test before shipping; the poll interval floor
  is the mitigation.

## Recommended plan

| Phase | Scope | Verifiable here? |
|---|---|---|
| 1 | Claude via `agents --json`: parser, tracker, widget line, tray text, tests; counts only | Yes: this machine has live sessions; the fake-CLI method used for alerts also works |
| 2 | Alerts for "needs input" and "finished" | Yes |
| 3 | Optional source B for exact "finished at" and zero idle cost (undocumented, guarded) | Yes |
| 4 | WSL bridge for Windows users (per-distro opt-in) | Yes on this machine |
| 5 | Codex hooks: helper subcommand, printed snippet, disclosure | Partly: needs a real Codex |
| 6 | Codex database fallback | No, until a Codex with an in-progress turn is observed |

Phases 1 and 2 deliver most of the value, rest on an official interface, and are fully testable here.

## Risks and open questions

- The `agents` feature is a research preview and may change. Mitigation: strict parser, unknown
  values become "unknown", a visible "activity unavailable" state, fixtures like the `/usage` ones.
- Idle sessions inflate the count (this machine has an old idle CLI session): show working and needs-input
  prominently, and idle only as a small count.
- Counting `claude-desktop`, `cli` and `sdk-cli` sessions together may surprise; consider a per-kind filter.
- Not yet checked: whether `agents --json` behaves the same on Windows `claude.exe` and macOS (docs do not
  state a minimum version for `--json`; it exists in 2.1.321/2.1.322).

## Decisions needed

1. **Scope of "session":** all live processes (recommended), or only background agents?
2. **Names:** counts only (recommended) or optional project folder names?
3. **Alerts:** build "needs input" and "finished" alerts in phase 2? (recommended: yes, off by default)
4. **Codex:** hooks first (needs the user to enable them), or wait until a real Codex is available?
5. **WSL:** is bridging WSL sessions needed on Windows? (Your own sessions here run in WSL.)
