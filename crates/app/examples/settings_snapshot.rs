//! Renders the settings window to a raw RGBA file for visual inspection.
//! Windows only: `settings_snapshot <out_dir>`. Not part of the shipped app.

#[cfg(not(windows))]
fn main() {
    eprintln!("settings_snapshot runs on Windows only");
}

#[cfg(windows)]
mod ui {
    use std::time::Duration;

    use slint::{ComponentHandle, Timer, TimerMode};

    slint::include_modules!();

    pub fn run(out: String) {
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
            .expect("backend");
        let w = SettingsWindow::new().unwrap();
        w.set_version("0.9.0".into());
        w.set_message("Couldn't change launch at login: example message".into());
        w.set_form(SettingsForm {
            claude_enabled: true,
            cli_path: "".into(),
            interval_index: 4,
            interval_minutes: 7,
            launch_at_login: true,
            update_check: false,
            update_beta: true,
            widget_visible: true,
            on_top: true,
            compact: false,
            theme_index: 0,
            opacity: 95,
            notify_enabled: true,
            warn: 75,
            critical: 90,
            exhausted: true,
            reset: true,
            errors: false,
            codex_enabled: true,
            codex_cli_path: "".into(),
            codex_notify_enabled: true,
            activity_enabled: true,
            activity_alert_needs_input: true,
            activity_alert_finished: false,
            activity_alert_min_secs: 60,
            activity_finished_minutes: 10,
            wsl_distros: "Ubuntu".into(),
            wsl_claude_path: "".into(),
        });
        w.set_show_wsl(true);
        w.window().set_size(slint::LogicalSize::new(400.0, 1250.0));
        w.show().unwrap();
        let weak = w.as_weak();
        let timer = Timer::default();
        timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(1200),
            move || {
                let w = weak.upgrade().unwrap();
                let snap = w.window().take_snapshot().expect("snapshot");
                std::fs::write(format!("{out}/settings.rgba"), snap.as_bytes()).unwrap();
                std::fs::write(
                    format!("{out}/settings.size"),
                    format!("{} {}", snap.width(), snap.height()),
                )
                .unwrap();
                let _ = slint::quit_event_loop();
            },
        );
        slint::run_event_loop().unwrap();
    }
}

#[cfg(windows)]
fn main() {
    ui::run(std::env::args().nth(1).expect("out dir"));
}
