# Contributing

Thanks for helping. A few ground rules keep this app trustworthy:

- **Never invent a number.** A metric is `Official`, `Measured` or `Estimated`, or it is absent.
  Unknown provider output must become an error, not a guess.
- **Read-only and local.** No network code, no reading credentials, no modifying CLI files, no
  conversation content, no telemetry.
- **Keep decisions testable.** What to show and when to alert belongs in plain Rust with unit tests
  (`view`, `widget`, `notify`, `form`, `store`); `shell.rs` stays a thin Windows/macOS shell. `unsafe` is
  allowed only in `crates/platform`.

## Workflow

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The project page lives in `site/` (plain HTML, no build step); see the Website section of
`docs/RELEASE_CHECKLIST.md`.

CI runs the same on Linux and Windows and macOS (the macOS jobs can be skipped with the `SKIP_MACOS` variable, see `docs/RELEASE_CHECKLIST.md`). GUI behaviour needs a real Windows session; say in the pull
request what you verified and how, and what you could not.

## Parser changes

If Claude Code's `/usage` wording changes, add a fixture under `tests/fixtures/claude/usage` (redact your
email, organisation and paths) and a test before touching the parser.

## New providers

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#adding-a-provider-for-example-codex-cli).
