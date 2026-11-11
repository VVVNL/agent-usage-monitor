# `claude agents --json` fixtures

| File | Provenance |
|---|---|
| agents_interactive.json | **Observed shape** (Claude Code 2.1.321/2.1.322, 2026-11-10): the keys and value types of a real call. Every value (paths, names, ids, pids, times) is replaced. |
| agents_waiting.json | **Docs-derived.** `status: "waiting"` with `waitingFor`, as documented on the Agent view page. Not yet seen live. |
| agents_background.json | **Docs-derived.** Background sessions with `id` and `state` (`working`, `blocked`, `done`, `failed`, `stopped`). Not yet seen live. |
| agents_unknown_status.json | Invented: a status value we do not know, to prove it never becomes "idle". |
| agents_empty.json | `[]`: no live sessions. |

Timestamps (`startedAt`) are Unix milliseconds. The real output also contains the project folder
(`cwd`) and conversation title (`name`); the parser deliberately does not read them.
