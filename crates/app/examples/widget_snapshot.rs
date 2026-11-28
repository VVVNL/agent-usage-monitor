//! Renders the widget in several states to raw RGBA files for visual inspection.
//! Windows only: `widget_snapshot <out_dir>`. Not part of the shipped app.

#[cfg(not(windows))]
fn main() {
    eprintln!("widget_snapshot runs on Windows only");
}

#[cfg(windows)]
mod ui {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

    slint::include_modules!();

    fn bar(label: &str, percent: i32, level: i32, detail: &str) -> BarData {
        BarData {
            label: label.into(),
            percent,
            level,
            detail: detail.into(),
            forecast: "".into(),
            forecast_level: 0,
            projected: -1,
        }
    }

    fn bar_with_forecast(
        label: &str,
        percent: i32,
        level: i32,
        detail: &str,
        forecast: &str,
        forecast_level: i32,
        projected: i32,
    ) -> BarData {
        BarData {
            forecast: forecast.into(),
            forecast_level,
            projected,
            ..bar(label, percent, level, detail)
        }
    }

    fn section(
        problem: &str,
        stale: bool,
        bars: Vec<BarData>,
        updated: &str,
        compact: &str,
    ) -> SectionData {
        SectionData {
            title: "Claude Code".into(),
            problem: problem.into(),
            updated: updated.into(),
            stale,
            compact: compact.into(),
            bars: ModelRc::new(VecModel::from(bars)),
            chips: ModelRc::new(VecModel::from(Vec::<ChipData>::new())),
            activity_level: 0,
            activity_busy: 0,
        }
    }

    fn with_activity(
        mut s: SectionData,
        chips: &[(&str, i32)],
        level: i32,
        busy: i32,
    ) -> SectionData {
        s.chips = ModelRc::new(VecModel::from(
            chips
                .iter()
                .map(|(t, l)| ChipData {
                    text: (*t).into(),
                    level: *l,
                })
                .collect::<Vec<_>>(),
        ));
        s.activity_level = level;
        s.activity_busy = busy;
        s
    }

    pub fn run(out: String) {
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
            .expect("backend");
        let w = Widget::new().unwrap();
        w.window()
            .set_position(slint::PhysicalPosition::new(120, 120));

        let healthy = vec![section(
            "",
            false,
            vec![
                bar_with_forecast(
                    "Session",
                    35,
                    0,
                    "resets in 4h 04m",
                    "Runs out in ~1h 44m",
                    2,
                    100,
                ),
                bar_with_forecast("Weekly", 82, 1, "resets in 4d 12h", "~99% by reset", 1, 99),
                bar_with_forecast(
                    "Weekly (Fable)",
                    60,
                    0,
                    "resets in 4d 12h",
                    "~84% by reset",
                    0,
                    84,
                ),
            ],
            "Updated 1 min ago",
            "Claude Code  35% · 82%",
        )];
        let busy = vec![with_activity(
            healthy[0].clone(),
            &[
                ("needs input: permission prompt", 4),
                ("2 working", 2),
                ("finished 4 min ago", 3),
            ],
            4,
            3,
        )];
        let idle = vec![with_activity(healthy[0].clone(), &[("2 idle", 1)], 1, 0)];
        let problem = vec![section(
            "Offline",
            true,
            vec![
                bar("Session", 35, 0, "resets in 2h 10m"),
                bar("Weekly", -1, 3, "reset passed, refreshing"),
            ],
            "Stale: updated 3h 55m ago",
            "Claude Code  35% · —",
        )];
        let login = vec![section(
            "Not logged in — run `claude` to sign in",
            false,
            vec![],
            "Never updated",
            "Claude Code  no data",
        )];

        // (name, dark, compact, sections)
        let steps: Vec<(&str, bool, bool, Vec<SectionData>)> = vec![
            ("dark_full", true, false, healthy.clone()),
            ("light_full", false, false, healthy.clone()),
            ("dark_compact", true, true, healthy),
            ("dark_activity", true, false, busy.clone()),
            ("light_activity", false, false, busy.clone()),
            ("dark_compact_activity", true, true, busy),
            ("dark_idle_sessions", true, false, idle),
            ("dark_problem", true, false, problem),
            ("dark_login", true, false, login),
        ];
        let steps = Rc::new(RefCell::new(steps.into_iter()));
        let timer = Rc::new(Timer::default());
        let weak = w.as_weak();
        let t2 = timer.clone();
        w.show().unwrap();
        timer.start(TimerMode::Repeated, Duration::from_millis(900), move || {
            let w = weak.upgrade().unwrap();
            // Capture what was configured on the previous tick, then configure the next.
            if let Some(name) = CURRENT.with(|c| c.borrow_mut().take()) {
                let snap = w.window().take_snapshot().expect("snapshot");
                let (sw, sh) = (snap.width(), snap.height());
                std::fs::write(format!("{out}/{name}.rgba"), snap.as_bytes()).unwrap();
                std::fs::write(format!("{out}/{name}.size"), format!("{sw} {sh}")).unwrap();
            }
            match steps.borrow_mut().next() {
                Some((name, dark, compact, sections)) => {
                    w.global::<Pal>().set_dark(dark);
                    w.set_compact(compact);
                    w.set_sections(ModelRc::new(VecModel::from(sections)));
                    CURRENT.with(|c| *c.borrow_mut() = Some(name.to_owned()));
                }
                None => {
                    t2.stop();
                    let _ = slint::quit_event_loop();
                }
            }
        });
        slint::run_event_loop().unwrap();
    }

    thread_local! { static CURRENT: RefCell<Option<String>> = const { RefCell::new(None) }; }
}

#[cfg(windows)]
fn main() {
    ui::run(std::env::args().nth(1).expect("out dir"));
}
