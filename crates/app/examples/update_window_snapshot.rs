//! Renders the update install window in its states to raw RGBA files for visual inspection.
//! Windows only: `update_window_snapshot <out_dir>`. Not part of the shipped app.

#[cfg(not(windows))]
fn main() {
    eprintln!("update_window_snapshot runs on Windows only");
}

#[cfg(windows)]
mod ui {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use slint::{ComponentHandle, Timer, TimerMode};

    slint::include_modules!();

    pub fn run(out: String) {
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
            .expect("backend");
        let states: [(&str, &str, &str, bool, bool, bool); 3] = [
            (
                "confirm",
                "Install version 0.12.0?",
                "The installer is downloaded from this project's GitHub release and checked against the project's signature before it runs. The widget closes and starts again when it is done. Your settings are kept.",
                false,
                false,
                false,
            ),
            (
                "busy",
                "Downloading version 0.12.0…",
                "Downloading the installer and checking its signature. This takes a few seconds.",
                true,
                false,
                false,
            ),
            (
                "failed",
                "Couldn't update",
                "the checksum list is not signed by the project's key.\n\nNothing was changed. You can install the new version by hand from the release page.",
                false,
                true,
                true,
            ),
        ];
        let windows: Rc<RefCell<Vec<(String, UpdateInstallWindow)>>> = Rc::default();
        for (name, heading, message, busy, done, page) in states {
            let w = UpdateInstallWindow::new().unwrap();
            w.set_heading(heading.into());
            w.set_message(message.into());
            w.set_busy(busy);
            w.set_done(done);
            w.set_show_page_button(page);
            w.show().unwrap();
            windows.borrow_mut().push((name.to_owned(), w));
        }
        let timer = Timer::default();
        let list = windows.clone();
        timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(1500),
            move || {
                for (name, w) in list.borrow().iter() {
                    let snap = w.window().take_snapshot().expect("snapshot");
                    std::fs::write(format!("{out}/upd_{name}.rgba"), snap.as_bytes()).unwrap();
                    std::fs::write(
                        format!("{out}/upd_{name}.size"),
                        format!("{} {}", snap.width(), snap.height()),
                    )
                    .unwrap();
                }
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
