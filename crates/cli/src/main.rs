//! Headless runner: proves the core works without any GUI.
//!
//! Usage: aum [--claude] [--codex] [--json] [--claude-path <path>] [--codex-path <path>]
//! With no provider flag, Claude Code is queried.

use std::path::PathBuf;
use std::process::ExitCode;

use aum_core::{ConnectionStatus, Provider, ProviderState, format_countdown};
use aum_providers::claude::ClaudeProvider;
use aum_providers::codex::CodexProvider;
use chrono::Utc;

const HELP: &str = "aum [--claude] [--codex] [--json] [--claude-path <path>] [--codex-path <path>]\n\
Prints current usage limits. With no provider flag, Claude Code is queried.";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let (mut json, mut want_claude, mut want_codex) = (false, false, false);
    let (mut claude_path, mut codex_path): (Option<PathBuf>, Option<PathBuf>) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--json" => json = true,
            "--claude" => want_claude = true,
            "--codex" => want_codex = true,
            "--claude-path" => claude_path = args.next().map(PathBuf::from),
            "--codex-path" => codex_path = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
    }
    if !want_claude && !want_codex {
        want_claude = true;
    }

    let mut providers: Vec<Box<dyn Provider>> = Vec::new();
    if want_claude {
        providers.push(Box::new(ClaudeProvider::new(claude_path)));
    }
    if want_codex {
        providers.push(Box::new(CodexProvider::new(codex_path)));
    }

    let mut states = Vec::new();
    for provider in &providers {
        let mut state = ProviderState::new(provider.id());
        state.last_attempt_at = Some(Utc::now());
        match provider.fetch().await {
            Ok(snap) => {
                state.status = ConnectionStatus::Connected;
                state.snapshot = Some(snap);
            }
            Err(e) => {
                state.status = e.status();
                state.consecutive_failures = 1;
            }
        }
        states.push(state);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&states).unwrap_or_default()
        );
    } else {
        states.iter().for_each(print_table);
    }
    if states
        .iter()
        .all(|s| matches!(s.status, ConnectionStatus::Connected))
    {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn print_table(st: &ProviderState) {
    let now = Utc::now();
    println!("{}  [{:?}]", st.provider.display_name(), st.status);
    let Some(snap) = &st.snapshot else {
        println!("  (no data)");
        return;
    };
    if let Some(plan) = &snap.plan {
        println!("  plan: {plan}");
    }
    for w in &snap.windows {
        let reset = w
            .time_to_reset(now)
            .map_or("reset time passed".to_owned(), |d| {
                format!("resets in {}", format_countdown(d))
            });
        println!(
            "  {:<22} {:>3}%  {:<18} [{:?}]",
            w.kind.label(),
            w.used_percent,
            reset,
            w.provenance
        );
    }
    for line in &snap.unrecognized {
        println!("  ! unrecognised: {line}");
    }
}
