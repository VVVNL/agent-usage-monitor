//! Application shell (Windows and macOS): Slint event loop, tray icon / menu bar item, floating widget.
//! Decisions about *what* to show live in `aum_app::{view, widget}` (tested);
//! native window tweaks live in `aum_platform`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aum_app::activity::{self, ActivityView, Tracker};
use aum_app::display::LayoutWatch;
use aum_app::form::{self, Form};
use aum_app::widget::{self, WidgetModel};
use aum_app::{icon, notify, view};
use aum_collector::activity::{ActivityConfig, ActivityService};
use aum_collector::{Collector, CollectorConfig};
use aum_core::ProviderId;
use aum_core::{ActivityHealth, ActivitySource};
use aum_core::{Provider, ProviderState};
use aum_providers::claude::ClaudeProvider;
use aum_providers::claude::activity::{ClaudeActivity, WslConfig};
use aum_providers::codex::CodexProvider;
use aum_providers::runner::SystemRunner;
use aum_store::{LoadOutcome, Paths, Settings, Theme, WidgetPos};
use aum_update::{UpdateState, Version};
use chrono::Utc;
#[cfg(windows)]
use slint::winit_030::winit::platform::windows::WindowExtWindows;
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

slint::include_modules!();

const INSTANCE_NAME: &str = "AgentUsageMonitor-single-instance";
const INTERVALS: [(u64, &str); 4] = [
    (60, "1 minute"),
    (300, "5 minutes"),
    (900, "15 minutes"),
    (1800, "30 minutes"),
];
const OPACITIES: [u8; 4] = [100, 85, 70, 50];
/// Countdown text is refreshed this often; no other periodic work runs.
const TICK: Duration = Duration::from_secs(60);
/// Wait this long after the last move before writing the position to disk.
const SAVE_POS_DELAY: Duration = Duration::from_millis(600);
/// How often the monitor layout is compared while the widget is visible. Plugging a display in
/// or out does not always produce a window event (when the widget is not touched by the change),
/// and the comparison is a handful of cheap system calls.
const DISPLAY_POLL: Duration = Duration::from_secs(2);
/// Logical size used to place the widget before its real size is known.
const ESTIMATED_SIZE: (f32, f32) = (250.0, 170.0);

/// Append a line to `diagnostics.log` in the settings folder (never secrets: only our own
/// status messages). Kept under 64 KB by starting over when it grows past that.
fn diag(msg: &str) {
    use std::io::Write;
    let Ok(paths) = Paths::system() else { return };
    let _ = std::fs::create_dir_all(&paths.dir);
    let file = paths.dir.join("diagnostics.log");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > 64 * 1024) {
        let _ = std::fs::remove_file(&file);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        let _ = writeln!(f, "{} {msg}", Utc::now().format("%Y-%m-%dT%H:%M:%SZ"));
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` on the app if it is not already borrowed (re-entrant events are dropped, not panicked on).
fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                f(app);
            }
        }
    });
}

/// Polling service plus the task that wakes the UI when data changes.
struct Backend {
    collector: Collector,
    /// Session activity polling; `None` when switched off or Claude is disabled.
    activity: Option<ActivityService>,
    watchers: Vec<JoinHandle<()>>,
}

impl Backend {
    fn start(rt: &Runtime, settings: &Settings, initial: Vec<ProviderState>) -> Self {
        let _guard = rt.enter();
        let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
        if settings.claude_enabled {
            providers.push(Arc::new(ClaudeProvider::new(
                settings.claude_cli_path.as_ref().map(PathBuf::from),
            )));
        }
        if settings.codex_enabled {
            providers.push(Arc::new(CodexProvider::new(
                settings.codex_cli_path.as_ref().map(PathBuf::from),
            )));
        }
        let cfg = CollectorConfig::default().with_interval(settings.poll_interval());
        let collector = Collector::start(providers, cfg, initial);
        let mut rx = collector.subscribe();
        let mut watchers = vec![rt.spawn(async move {
            while rx.changed().await.is_ok() {
                if slint::invoke_from_event_loop(|| with_app(App::on_state_changed)).is_err() {
                    break; // event loop is gone
                }
            }
        })];

        // Session activity is Claude-only for now (Codex has no listing interface).
        let activity = (settings.claude_enabled && settings.activity.enabled).then(|| {
            let wsl = WslConfig::new(
                settings.activity.wsl_distros.clone(),
                settings.activity.wsl_claude_path.clone(),
            );
            let source: Arc<dyn ActivitySource> = Arc::new(ClaudeActivity::new(
                settings.claude_cli_path.as_ref().map(PathBuf::from),
                wsl,
            ));
            ActivityService::start(vec![source], ActivityConfig::default())
        });
        if let Some(svc) = &activity {
            let mut rx = svc.subscribe();
            watchers.push(rt.spawn(async move {
                while rx.changed().await.is_ok() {
                    if slint::invoke_from_event_loop(|| with_app(App::on_activity_changed)).is_err()
                    {
                        break;
                    }
                }
            }));
        }
        Self {
            collector,
            activity,
            watchers,
        }
    }

    fn refresh_now(&self) {
        self.collector.refresh_now();
        if let Some(a) = &self.activity {
            a.refresh_now();
        }
    }

    fn stop(self, rt: &Runtime) {
        for w in &self.watchers {
            w.abort();
        }
        rt.block_on(async {
            if let Some(a) = self.activity {
                a.shutdown().await;
            }
            self.collector.shutdown().await;
        });
    }
}

/// Menu items that can be updated in place.
struct MenuState {
    /// One entry per section: how many rows it had (the menu's structure).
    signature: Vec<usize>,
    lines: Vec<MenuItem>,
    intervals: Vec<(u64, CheckMenuItem)>,
    /// "Custom…" entry: checked, and labelled with the value, when the interval is not a preset.
    custom_interval: CheckMenuItem,
    /// "Check for updates…", or "Update available: …" once one is known.
    update_item: MenuItem,
    show: CheckMenuItem,
    on_top: CheckMenuItem,
    compact: CheckMenuItem,
    click_through: CheckMenuItem,
    opacities: Vec<(u8, CheckMenuItem)>,
    themes: Vec<(Theme, CheckMenuItem)>,
}

impl MenuState {
    /// Make every check mark agree with the settings.
    fn sync(&self, s: &Settings) {
        self.show.set_checked(s.widget_visible);
        self.on_top.set_checked(s.widget_always_on_top);
        self.compact.set_checked(s.widget_compact);
        self.click_through.set_checked(s.widget_click_through);
        let current = s.poll_interval().as_secs();
        for (secs, item) in &self.intervals {
            item.set_checked(*secs == current);
        }
        let is_custom = !self.intervals.iter().any(|(secs, _)| *secs == current);
        self.custom_interval.set_checked(is_custom);
        self.custom_interval.set_text(if is_custom {
            format!("Custom: {}…", form::interval_label(current))
        } else {
            "Custom…".to_owned()
        });
        for (pct, item) in &self.opacities {
            item.set_checked(*pct == s.opacity_percent());
        }
        for (theme, item) in &self.themes {
            item.set_checked(*theme == s.widget_theme);
        }
    }
}

/// Text and enabled state of the tray's update entry.
fn update_menu_text(state: &UpdateState, s: &Settings, busy: bool) -> (String, bool) {
    if busy {
        return ("Checking for updates…".to_owned(), false);
    }
    match state.available(&current_version(), s.update.include_beta) {
        Some(a) => (
            format!("Update available: {} — open release page", a.version),
            true,
        ),
        None => ("Check for updates…".to_owned(), true),
    }
}

fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is a version")
}

/// Open a web page in the default browser. Only ever called with a URL we built ourselves.
fn open_url(url: &str) {
    #[cfg(windows)]
    let opener = "explorer.exe";
    #[cfg(target_os = "macos")]
    let opener = "open";
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

fn flatten(model: &view::TrayModel, notice: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(n) = notice {
        out.push(format!("⚠ {n}"));
    }
    for s in &model.sections {
        out.push(format!("{} — {}", s.title, s.status));
        out.extend(s.rows.iter().map(|r| format!("   {r}")));
        if let Some(a) = &s.activity {
            out.push(format!("   Sessions: {a}"));
        }
        out.push(format!("   {}", s.updated));
    }
    out
}

fn check(id: &str, label: &str) -> CheckMenuItem {
    CheckMenuItem::with_id(id, label, true, false, None)
}

fn build_menu(
    model: &view::TrayModel,
    notice: Option<&str>,
    settings: &Settings,
    update: &(String, bool),
) -> (Menu, MenuState) {
    let menu = Menu::new();
    let mut lines = Vec::new();
    for text in flatten(model, notice) {
        let item = MenuItem::new(text, false, None);
        let _ = menu.append(&item);
        lines.push(item);
    }
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("refresh", "Refresh now", true, None));

    let show = check("widget:show", "Show widget");
    let _ = menu.append(&show);

    let widget_menu = Submenu::new("Widget", true);
    let on_top = check("widget:ontop", "Always on top");
    let compact = check("widget:compact", "Compact mode");
    let click_through = check("widget:clickthrough", "Click-through (turn off here)");
    let _ = widget_menu.append(&on_top);
    let _ = widget_menu.append(&compact);
    let _ = widget_menu.append(&click_through);

    let opacity_menu = Submenu::new("Opacity", true);
    let mut opacities = Vec::new();
    for pct in OPACITIES {
        let item = check(&format!("widget:opacity:{pct}"), &format!("{pct}%"));
        let _ = opacity_menu.append(&item);
        opacities.push((pct, item));
    }
    let _ = widget_menu.append(&opacity_menu);

    let theme_menu = Submenu::new("Theme", true);
    let mut themes = Vec::new();
    for (theme, id, label) in [
        (Theme::Dark, "dark", "Dark"),
        (Theme::Light, "light", "Light"),
    ] {
        let item = check(&format!("widget:theme:{id}"), label);
        let _ = theme_menu.append(&item);
        themes.push((theme, item));
    }
    let _ = widget_menu.append(&theme_menu);
    let _ = menu.append(&widget_menu);

    let sub = Submenu::new("Refresh interval", true);
    let mut intervals = Vec::new();
    for (secs, label) in INTERVALS {
        let item = check(&format!("interval:{secs}"), label);
        let _ = sub.append(&item);
        intervals.push((secs, item));
    }
    // Any other value is entered in Settings; this entry opens it.
    let custom_interval = check("interval:custom", "Custom…");
    let _ = sub.append(&custom_interval);
    let _ = menu.append(&sub);
    let _ = menu.append(&MenuItem::with_id("settings", "Settings…", true, None));
    let _ = menu.append(&MenuItem::with_id(
        "open_dir",
        "Open settings folder",
        true,
        None,
    ));
    let update_item = MenuItem::with_id("update", &update.0, update.1, None);
    let _ = menu.append(&update_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("quit", "Quit", true, None));

    let state = MenuState {
        signature: model.sections.iter().map(|s| s.rows.len()).collect(),
        lines,
        intervals,
        custom_interval,
        update_item,
        show,
        on_top,
        compact,
        click_through,
        opacities,
        themes,
    };
    state.sync(settings);
    (menu, state)
}

fn make_icon(model: &view::TrayModel) -> Icon {
    Icon::from_rgba(
        icon::render_badged(
            model.icon_percent,
            model.icon_level,
            model.icon_stale,
            model.badge,
        ),
        icon::SIZE,
        icon::SIZE,
    )
    .expect("icon buffer has the right size")
}

fn level_int(l: activity::Level) -> i32 {
    match l {
        activity::Level::None => 0,
        activity::Level::Idle => 1,
        activity::Level::Working => 2,
        activity::Level::Finished => 3,
        activity::Level::NeedsInput => 4,
    }
}

fn to_section_data(m: &WidgetModel) -> Vec<SectionData> {
    m.sections
        .iter()
        .map(|s| SectionData {
            title: s.title.as_str().into(),
            problem: s.problem.as_str().into(),
            updated: s.updated.as_str().into(),
            stale: s.stale,
            compact: s.compact.as_str().into(),
            bars: ModelRc::new(VecModel::from(
                s.bars
                    .iter()
                    .map(|b| BarData {
                        label: b.label.as_str().into(),
                        percent: b.percent.map_or(-1, i32::from),
                        level: match b.level {
                            icon::Level::Ok => 0,
                            icon::Level::Warn => 1,
                            icon::Level::Critical => 2,
                            icon::Level::Unknown => 3,
                        },
                        detail: b.detail.as_str().into(),
                    })
                    .collect::<Vec<_>>(),
            )),
            chips: ModelRc::new(VecModel::from(
                s.chips
                    .iter()
                    .map(|c| ChipData {
                        text: c.text.as_str().into(),
                        level: level_int(c.level),
                    })
                    .collect::<Vec<_>>(),
            )),
            activity_level: level_int(s.activity_level),
            activity_busy: s.activity_busy as i32,
        })
        .collect()
}

fn to_slint_form(f: &Form) -> SettingsForm {
    SettingsForm {
        claude_enabled: f.claude_enabled,
        cli_path: f.cli_path.as_str().into(),
        interval_index: form::preset_index(f.interval_minutes),
        interval_minutes: f.interval_minutes,
        launch_at_login: f.launch_at_login,
        update_check: f.update_check,
        update_beta: f.update_beta,
        widget_visible: f.widget_visible,
        on_top: f.on_top,
        compact: f.compact,
        theme_index: f.theme_index,
        opacity: f.opacity,
        notify_enabled: f.notify_enabled,
        warn: f.warn,
        critical: f.critical,
        exhausted: f.exhausted,
        reset: f.reset,
        errors: f.errors,
        codex_enabled: f.codex_enabled,
        codex_cli_path: f.codex_cli_path.as_str().into(),
        codex_notify_enabled: f.codex_notify_enabled,
        activity_enabled: f.activity_enabled,
        activity_alert_needs_input: f.activity_alert_needs_input,
        activity_alert_finished: f.activity_alert_finished,
        activity_alert_min_secs: f.activity_alert_min_secs,
        activity_finished_minutes: f.activity_finished_minutes,
        wsl_distros: f.wsl_distros.as_str().into(),
        wsl_claude_path: f.wsl_claude_path.as_str().into(),
    }
}

fn from_slint_form(f: &SettingsForm) -> Form {
    Form {
        claude_enabled: f.claude_enabled,
        cli_path: f.cli_path.to_string(),
        interval_minutes: f.interval_minutes,
        launch_at_login: f.launch_at_login,
        update_check: f.update_check,
        update_beta: f.update_beta,
        widget_visible: f.widget_visible,
        on_top: f.on_top,
        compact: f.compact,
        theme_index: f.theme_index,
        opacity: f.opacity,
        notify_enabled: f.notify_enabled,
        warn: f.warn,
        critical: f.critical,
        exhausted: f.exhausted,
        reset: f.reset,
        errors: f.errors,
        codex_enabled: f.codex_enabled,
        codex_cli_path: f.codex_cli_path.to_string(),
        codex_notify_enabled: f.codex_notify_enabled,
        activity_enabled: f.activity_enabled,
        activity_alert_needs_input: f.activity_alert_needs_input,
        activity_alert_finished: f.activity_alert_finished,
        activity_alert_min_secs: f.activity_alert_min_secs,
        activity_finished_minutes: f.activity_finished_minutes,
        wsl_distros: f.wsl_distros.to_string(),
        wsl_claude_path: f.wsl_claude_path.to_string(),
    }
}

struct App {
    rt: Runtime,
    paths: Paths,
    settings: Settings,
    notice: Option<String>,
    backend: Option<Backend>,
    tray: Option<TrayIcon>,
    menu: Option<MenuState>,
    widget: Widget,
    last_widget_model: Option<WidgetModel>,
    _tick: Timer,
    retry: Timer,
    /// Second pass after a display change, once the system has finished moving windows.
    settle: Timer,
    /// Watches the monitor layout while the widget is visible (see `DISPLAY_POLL`).
    display_poll: Timer,
    /// What the update check remembers (`update.json`); nothing is downloaded or installed.
    update: UpdateState,
    update_busy: bool,
    /// Hourly "is the daily check due?" timer, and the one-shot first check after start.
    update_timer: Timer,
    update_first: Timer,
    /// Fires once after start to ask the one-time update question.
    prompt_timer: Timer,
    prompt_win: Option<UpdatePromptWindow>,
    /// Last monitor layout seen; lets us tell display changes from the user dragging the widget.
    layout: Rc<RefCell<LayoutWatch>>,
    notifier: notify::Engine,
    settings_win: Option<SettingsWindow>,
    /// One tracker per provider: it turns activity snapshots into events and "finished" memory.
    trackers: HashMap<ProviderId, Tracker>,
}

impl App {
    fn states(&self) -> Vec<ProviderState> {
        self.backend
            .as_ref()
            .map(|b| b.collector.snapshot())
            .unwrap_or_default()
    }

    fn save_settings(&self) {
        let _ = aum_store::save_settings(&self.paths, &self.settings);
    }

    fn persist_cache(&self) {
        // Best effort: a failed cache write must never disturb the app.
        let _ = aum_store::save_cache(&self.paths, &self.states());
    }

    fn on_state_changed(&mut self) {
        self.persist_cache();
        self.send_notifications();
        self.render();
    }

    /// Evaluate alert rules against the new state and show any toasts (off the UI thread).
    fn send_notifications(&mut self) {
        let (claude, codex) = (
            self.settings.claude_notify.clone(),
            self.settings.codex_notify.clone(),
        );
        let now = Utc::now();
        let notes = self.notifier.evaluate(
            &self.states(),
            |id| match id {
                ProviderId::Codex => codex.clone(),
                _ => claude.clone(),
            },
            now,
        );
        if notes.is_empty() {
            return;
        }
        let _ = aum_store::save_notified(&self.paths, &self.notifier.export(now));
        Self::show_toasts(notes);
    }

    /// Show notifications off the UI thread, logging each title (never any content).
    fn show_toasts(notes: Vec<notify::Notification>) {
        for n in notes {
            diag(&format!("alert: {}", n.title));
            std::thread::spawn(move || {
                if let Err(e) = aum_platform::toast(&n.title, &n.body) {
                    diag(&format!("toast failed: {e}"));
                }
            });
        }
    }

    fn activity_states(&self) -> Vec<aum_core::ActivityState> {
        self.backend
            .as_ref()
            .and_then(|b| b.activity.as_ref())
            .map(|a| a.snapshot())
            .unwrap_or_default()
    }

    /// What to show for each provider whose activity is switched on.
    fn activity_views(&self, now: chrono::DateTime<Utc>) -> HashMap<ProviderId, ActivityView> {
        if !self.settings.activity.enabled {
            return HashMap::new();
        }
        let ttl = self.settings.activity.finished_ttl();
        let empty = Tracker::new();
        self.activity_states()
            .iter()
            .map(|st| {
                let tracker = self.trackers.get(&st.provider).unwrap_or(&empty);
                (st.provider, activity::view(Some(st), tracker, now, ttl))
            })
            .collect()
    }

    /// New activity data: detect transitions, send alerts, redraw.
    fn on_activity_changed(&mut self) {
        let now = Utc::now();
        let cfg = self.settings.activity.clone();
        for st in self.activity_states() {
            // An unreadable source is "no information", never "idle".
            let snap = if st.health == ActivityHealth::Ok {
                st.snapshot.as_ref()
            } else {
                None
            };
            let tracker = self.trackers.entry(st.provider).or_default();
            let events = tracker.observe(snap, now);
            tracker.prune(now, cfg.finished_ttl());
            let notes = self.notifier.activity(st.provider, &events, &cfg, now);
            Self::show_toasts(notes);
        }
        self.render();
    }

    /// Re-render tray icon, tooltip, menu and (if visible) the widget.
    fn render(&mut self) {
        let states = self.states();
        if let Some(tray) = &self.tray {
            let now = Utc::now();
            let model = view::build_with_activity(
                &states,
                &self.activity_views(now),
                now,
                self.settings.poll_interval(),
            );
            let _ = tray.set_tooltip(Some(&model.tooltip));
            // macOS shows text next to the menu bar icon; Windows has no such thing.
            #[cfg(target_os = "macos")]
            tray.set_title(Some(&model.title));
            let _ = tray.set_icon(Some(make_icon(&model)));

            let texts = flatten(&model, self.notice.as_deref());
            let signature: Vec<usize> = model.sections.iter().map(|s| s.rows.len()).collect();
            let same_shape = self
                .menu
                .as_ref()
                .is_some_and(|m| m.lines.len() == texts.len() && m.signature == signature);
            if same_shape {
                // Update text in place; never replace a menu that may be open.
                if let Some(m) = &self.menu {
                    for (item, text) in m.lines.iter().zip(texts) {
                        item.set_text(text);
                    }
                }
            } else {
                let update = update_menu_text(&self.update, &self.settings, self.update_busy);
                let (menu, state) =
                    build_menu(&model, self.notice.as_deref(), &self.settings, &update);
                tray.set_menu(Some(Box::new(menu)));
                self.menu = Some(state);
            }
        }
        if self.settings.widget_visible {
            self.refresh_widget(false);
        }
    }

    fn refresh_widget(&mut self, force: bool) {
        let now = Utc::now();
        let model = widget::build_with_activity(
            &self.states(),
            &self.activity_views(now),
            now,
            self.settings.poll_interval(),
        );
        if !force && self.last_widget_model.as_ref() == Some(&model) {
            return; // nothing changed: no repaint
        }
        self.widget
            .set_sections(ModelRc::new(VecModel::from(to_section_data(&model))));
        self.last_widget_model = Some(model);
    }

    fn apply_widget_settings(&mut self) {
        self.widget.set_compact(self.settings.widget_compact);
        self.widget.set_on_top(self.settings.widget_always_on_top);
        self.widget
            .global::<Pal>()
            .set_dark(self.settings.widget_theme == Theme::Dark);
        // Changing the window level makes the toolkit reset the native window style
        // (observed on Windows: the layered-window opacity is lost), so re-apply our
        // tweaks once it has had a chance to run.
        self.retry
            .start(TimerMode::SingleShot, Duration::from_millis(150), || {
                with_app(|a| a.apply_native(0))
            });
    }

    /// Native tweaks that need the OS window. Retried briefly if it does not exist yet.
    fn apply_native(&mut self, attempts_left: u8) {
        let opacity = self.settings.opacity_percent();
        let hittest = !self.settings.widget_click_through;
        let done = self.widget.window().with_winit_window(|ww| {
            #[cfg(windows)]
            ww.set_skip_taskbar(true);
            let _ = ww.set_cursor_hittest(hittest);
            if let Ok(handle) = ww.window_handle() {
                // The native handle as an integer: HWND on Windows, NSView on macOS.
                let native = match handle.as_raw() {
                    #[cfg(windows)]
                    RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                    #[cfg(target_os = "macos")]
                    RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as isize),
                    _ => None,
                };
                if let Some(native) = native {
                    aum_platform::set_opacity(native, opacity);
                    aum_platform::round_corners(native);
                }
            }
        });
        if done.is_none() && attempts_left > 0 {
            self.retry.start(
                TimerMode::SingleShot,
                Duration::from_millis(50),
                move || {
                    with_app(|a| a.apply_native(attempts_left - 1));
                },
            );
        }
    }

    /// The monitor layout according to winit, primary first, with room left at the top for the
    /// macOS menu bar. `None` if the native window does not exist yet.
    fn winit_monitors(&self) -> Option<Vec<aum_store::Rect>> {
        self.widget.window().with_winit_window(|ww| {
            let primary = ww.primary_monitor();
            let mut all: Vec<_> = ww.available_monitors().collect();
            // Stable sort puts the primary monitor first and keeps the rest in order.
            all.sort_by_key(|m| Some(m) != primary.as_ref());
            all.iter()
                .map(|m| {
                    let (p, s) = (m.position(), m.size());
                    // Keep clear of the menu bar (about 38 points including the notch).
                    let inset = (38.0 * m.scale_factor()) as i32;
                    aum_store::Rect::new(p.x, p.y + inset, s.width as i32, s.height as i32 - inset)
                })
                .collect()
        })
    }

    /// The monitor layout now: the platform module where it can tell, otherwise winit.
    fn layout_now(&self) -> Vec<aum_store::Rect> {
        let found = aum_platform::monitors();
        if found.is_empty() {
            self.winit_monitors().unwrap_or_default()
        } else {
            found
        }
    }

    /// A monitor was plugged in or removed, or the resolution or scaling changed. Windows moves
    /// the widget by itself in that case and may drop our native tweaks, so: put it where the
    /// user's saved position says it belongs (or the default corner if that is gone), re-apply
    /// opacity and rounding, and repaint. Run again once things have settled.
    fn on_display_change(&mut self) {
        if !self.settings.widget_visible {
            return;
        }
        diag("display layout changed");
        self.reposition_widget();
        self.apply_widget_settings();
        self.refresh_widget(true);
        self.settle
            .start(TimerMode::SingleShot, Duration::from_millis(1500), || {
                with_app(|a| {
                    a.reposition_widget();
                    a.apply_native(0);
                    a.refresh_widget(true);
                })
            });
    }

    /// Move the widget to where the saved position (not wherever the system last put it) says,
    /// if that spot is reachable on the current monitors.
    fn reposition_widget(&mut self) {
        let monitors = self.layout_now();
        if monitors.is_empty() {
            return;
        }
        let size = self.widget.window().size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let pos = aum_store::place_widget(
            self.settings.widget_pos,
            (size.width as i32, size.height as i32),
            &monitors,
        );
        let target = slint::PhysicalPosition::new(pos.x, pos.y);
        if self.widget.window().position() != target {
            self.widget.window().set_position(target);
        }
    }

    /// Called every `DISPLAY_POLL` while the widget is visible: catches display changes that
    /// produced no window event.
    fn check_display_layout(&mut self) {
        if !self.settings.widget_visible {
            return;
        }
        let now = self.layout_now();
        let changed = self.layout.borrow_mut().observe(&now, Instant::now());
        if changed {
            self.on_display_change();
        }
    }

    fn show_widget(&mut self) {
        self.apply_widget_settings();
        self.refresh_widget(true);

        let scale = self.widget.window().scale_factor();
        let size = self.widget.window().size();
        let (w, h) = if size.width > 0 && size.height > 0 {
            (size.width as i32, size.height as i32)
        } else {
            (
                (ESTIMATED_SIZE.0 * scale) as i32,
                (ESTIMATED_SIZE.1 * scale) as i32,
            )
        };
        let monitors = aum_platform::monitors();
        let pos = aum_store::place_widget(self.settings.widget_pos, (w, h), &monitors);
        self.widget
            .window()
            .set_position(slint::PhysicalPosition::new(pos.x, pos.y));

        let _ = self.widget.show();
        if monitors.is_empty() {
            // This platform module cannot list monitors (macOS): ask winit now that the
            // native window exists, and place the widget again.
            if let Some(found) = self.winit_monitors() {
                let pos = aum_store::place_widget(self.settings.widget_pos, (w, h), &found);
                self.widget
                    .window()
                    .set_position(slint::PhysicalPosition::new(pos.x, pos.y));
            }
        }
        self.apply_native(5);
        self.display_poll
            .start(TimerMode::Repeated, DISPLAY_POLL, || {
                with_app(App::check_display_layout)
            });
    }

    fn set_widget_visible(&mut self, visible: bool) {
        self.settings.widget_visible = visible;
        self.save_settings();
        if visible {
            self.show_widget();
        } else {
            self.display_poll.stop();
            let _ = self.widget.hide();
        }
        self.sync_menu();
    }

    fn sync_menu(&self) {
        if let Some(m) = &self.menu {
            m.sync(&self.settings);
        }
    }

    fn save_position(&mut self, pos: WidgetPos) {
        if self.settings.widget_pos != Some(pos) {
            self.settings.widget_pos = Some(pos);
            self.save_settings();
        }
    }

    fn set_interval(&mut self, secs: u64) {
        if self.settings.poll_interval_secs == secs {
            return;
        }
        self.settings.poll_interval_secs = secs;
        self.save_settings();
        self.restart_backend();
    }

    /// Re-create the polling service after a provider or interval change.
    fn restart_backend(&mut self) {
        let initial = self.states();
        if let Some(old) = self.backend.take() {
            old.stop(&self.rt);
        }
        self.backend = Some(Backend::start(&self.rt, &self.settings, initial));
    }

    fn open_settings(&mut self) {
        if let Some(w) = &self.settings_win {
            let _ = w.show(); // already open: bring it forward
            return;
        }
        let Ok(win) = SettingsWindow::new() else {
            return;
        };
        win.set_form(to_slint_form(&Form::from_settings(
            &self.settings,
            aum_platform::autostart_enabled(),
        )));
        win.set_version(env!("CARGO_PKG_VERSION").into());
        win.set_show_wsl(cfg!(windows));
        win.on_changed(|f| {
            let form = from_slint_form(&f);
            with_app(|a| a.apply_form(form));
        });
        win.on_test_notification(|| {
            std::thread::spawn(|| {
                let result =
                    aum_platform::toast("Agent Usage Monitor", "Notifications are working.");
                if let Err(e) = &result {
                    diag(&format!("test toast failed: {e}"));
                }
                let msg = result
                    .err()
                    .map(|e| format!("Couldn't show a notification: {e}"));
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|a| a.set_settings_message(msg.unwrap_or_default()));
                });
            });
        });
        win.window().on_close_requested(|| {
            // Drop the window (and its memory) once it has closed.
            let _ = slint::invoke_from_event_loop(|| with_app(|a| a.settings_win = None));
            slint::CloseRequestResponse::HideWindow
        });
        let _ = win.show();
        self.settings_win = Some(win);
    }

    fn set_settings_message(&self, msg: String) {
        if let Some(w) = &self.settings_win {
            w.set_message(msg.into());
        }
    }

    /// Show the current settings in the settings window (after a change made elsewhere).
    fn refresh_settings_form(&self) {
        if let Some(w) = &self.settings_win {
            w.set_form(to_slint_form(&Form::from_settings(
                &self.settings,
                aum_platform::autostart_enabled(),
            )));
        }
    }

    /// Apply the settings window's form: validate, persist, and act on what changed.
    fn apply_form(&mut self, form: Form) {
        let old = self.settings.clone();
        form.apply_to(&mut self.settings);
        if self.settings == old {
            return;
        }
        let mut message = String::new();

        if self.settings.launch_at_login != old.launch_at_login {
            let result = std::env::current_exe()
                .map_err(|e| e.to_string())
                .and_then(|exe| aum_platform::set_autostart(self.settings.launch_at_login, &exe));
            if let Err(e) = result {
                self.settings.launch_at_login = old.launch_at_login;
                diag(&format!("launch at login change failed: {e}"));
                message = format!("Couldn't change launch at login: {e}");
            }
        }
        self.save_settings();
        if self.settings.update.check_daily != old.update.check_daily {
            self.schedule_update_checks(self.settings.update.check_daily);
        }
        if self.settings.update.include_beta != old.update.include_beta {
            self.refresh_update_menu();
        }

        let s = &self.settings;
        if s.claude_enabled != old.claude_enabled
            || s.claude_cli_path != old.claude_cli_path
            || s.codex_enabled != old.codex_enabled
            || s.codex_cli_path != old.codex_cli_path
            || s.poll_interval_secs != old.poll_interval_secs
            || s.activity.enabled != old.activity.enabled
            || s.activity.wsl_distros != old.activity.wsl_distros
            || s.activity.wsl_claude_path != old.activity.wsl_claude_path
        {
            if !s.activity.enabled {
                self.trackers.clear();
            }
            self.restart_backend();
        }
        if self.settings.widget_visible != old.widget_visible {
            let visible = self.settings.widget_visible;
            self.set_widget_visible(visible);
        }
        let s = &self.settings;
        if s.widget_always_on_top != old.widget_always_on_top
            || s.widget_compact != old.widget_compact
            || s.widget_theme != old.widget_theme
        {
            self.apply_widget_settings();
            self.refresh_widget(true);
        }
        if self.settings.opacity_percent() != old.opacity_percent() {
            self.apply_native(0);
        }

        self.set_settings_message(message);
        self.refresh_settings_form();
        self.sync_menu();
        self.render();
    }

    // ----- update notifications (see docs/research/auto-update.md) -----

    fn refresh_update_menu(&self) {
        if let Some(m) = &self.menu {
            let (text, enabled) = update_menu_text(&self.update, &self.settings, self.update_busy);
            m.update_item.set_text(text);
            m.update_item.set_enabled(enabled);
        }
    }

    /// (Re)arm the automatic check: an hourly "is it due?" tick plus one first check shortly after
    /// start (spread over a few minutes so many installs do not ask at the same second).
    fn schedule_update_checks(&mut self, soon: bool) {
        self.update_timer.stop();
        self.update_first.stop();
        if !self.settings.update.check_daily {
            return;
        }
        self.update_timer
            .start(TimerMode::Repeated, Duration::from_secs(3600), || {
                with_app(App::auto_update_tick)
            });
        let jitter = u64::from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos())
                % 240,
        );
        let delay = if soon { 3 } else { 60 + jitter };
        self.update_first
            .start(TimerMode::SingleShot, Duration::from_secs(delay), || {
                with_app(App::auto_update_tick)
            });
    }

    fn auto_update_tick(&mut self) {
        if self.settings.update.check_daily && self.update.is_due(Utc::now()) {
            self.start_update_check(false);
        }
    }

    /// `manual`: the user asked (always answered with a notification). Automatic checks only ever
    /// notify about a version they have not announced before.
    fn start_update_check(&mut self, manual: bool) {
        if self.update_busy {
            return;
        }
        let now = Utc::now();
        let include_beta = self.settings.update.include_beta;
        if manual && !self.update.manual_check_allowed(now) {
            // Asked a moment ago: answer from memory instead of sending another request.
            let report = aum_update::Report {
                available: self.update.available(&current_version(), include_beta),
                error: self.update.last_error.clone(),
            };
            self.tell_update_result(&report);
            return;
        }
        self.update_busy = true;
        self.refresh_update_menu();
        let mut state = self.update.clone();
        self.rt.spawn(async move {
            let report = aum_update::check(
                &SystemRunner,
                &aum_update::fetch::curl_path(),
                &mut state,
                &current_version(),
                include_beta,
                Utc::now(),
            )
            .await;
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|a| a.update_done(state, &report, manual));
            });
        });
    }

    fn update_done(&mut self, state: UpdateState, report: &aum_update::Report, manual: bool) {
        self.update = state;
        self.update_busy = false;
        if let Some(e) = &report.error {
            diag(&format!("update check failed: {e}"));
        }
        let announce = match &report.available {
            Some(a) => manual || self.update.announced.as_deref() != Some(a.tag.as_str()),
            None => manual,
        };
        if let Some(a) = &report.available
            && announce
        {
            self.update.announced = Some(a.tag.clone());
        }
        self.update.save(&self.paths);
        self.refresh_update_menu();
        if announce {
            self.tell_update_result(report);
        }
    }

    fn tell_update_result(&self, report: &aum_update::Report) {
        let msg = match (&report.available, &report.error) {
            (Some(a), _) => format!(
                "Version {} is available. Choose \"Update available\" in the tray menu to open its page.",
                a.version
            ),
            (None, Some(e)) => format!("Couldn't check for updates: {e}."),
            (None, None) => format!("You're up to date (version {}).", current_version()),
        };
        std::thread::spawn(move || {
            if let Err(e) = aum_platform::toast("Agent Usage Monitor", &msg) {
                diag(&format!("update toast failed: {e}"));
            }
        });
    }

    fn open_update_page(&self) {
        let found = self
            .update
            .available(&current_version(), self.settings.update.include_beta);
        if let Some(url) = found.and_then(|a| aum_update::release_page(&a.tag)) {
            open_url(&url);
        }
    }

    /// The one-time question. Closing the window counts as "no": it is asked once, never nagged.
    fn maybe_prompt_for_updates(&mut self) {
        if self.settings.update.prompted || self.prompt_win.is_some() {
            return;
        }
        let Ok(win) = UpdatePromptWindow::new() else {
            return;
        };
        win.on_answered(|yes| with_app(|a| a.answer_update_prompt(yes)));
        win.window().on_close_requested(|| {
            let _ = slint::invoke_from_event_loop(|| with_app(|a| a.answer_update_prompt(false)));
            slint::CloseRequestResponse::HideWindow
        });
        let _ = win.show();
        self.prompt_win = Some(win);
    }

    fn answer_update_prompt(&mut self, yes: bool) {
        let Some(win) = self.prompt_win.take() else {
            return; // already answered (the close handler runs after a button too)
        };
        let _ = win.hide();
        self.settings.update.prompted = true;
        self.settings.update.check_daily = yes;
        self.save_settings();
        self.refresh_settings_form();
        self.schedule_update_checks(yes);
    }

    fn on_menu(&mut self, id: &str) {
        match id {
            "update" => {
                if self
                    .update
                    .available(&current_version(), self.settings.update.include_beta)
                    .is_some()
                {
                    self.open_update_page();
                } else {
                    self.start_update_check(true);
                }
            }
            "refresh" => self.refresh(),
            "settings" | "interval:custom" => self.open_settings(),
            "open_dir" => {
                let _ = std::fs::create_dir_all(&self.paths.dir);
                #[cfg(windows)]
                let opener = "explorer.exe";
                #[cfg(target_os = "macos")]
                let opener = "open";
                let _ = std::process::Command::new(opener)
                    .arg(&self.paths.dir)
                    .spawn();
            }
            "quit" => {
                let _ = slint::quit_event_loop();
                return;
            }
            "widget:show" => {
                let visible = !self.settings.widget_visible;
                self.set_widget_visible(visible);
            }
            "widget:ontop" => {
                self.settings.widget_always_on_top = !self.settings.widget_always_on_top;
                self.save_settings();
                self.apply_widget_settings();
            }
            "widget:compact" => {
                self.settings.widget_compact = !self.settings.widget_compact;
                self.save_settings();
                self.apply_widget_settings();
                self.refresh_widget(true);
            }
            "widget:clickthrough" => {
                self.settings.widget_click_through = !self.settings.widget_click_through;
                self.save_settings();
                self.apply_native(0);
            }
            other => {
                if let Some(p) = other
                    .strip_prefix("widget:opacity:")
                    .and_then(|s| s.parse::<u8>().ok())
                {
                    self.settings.widget_opacity_percent = p;
                    self.save_settings();
                    self.apply_native(0);
                } else if let Some(t) = other.strip_prefix("widget:theme:") {
                    self.settings.widget_theme = if t == "light" {
                        Theme::Light
                    } else {
                        Theme::Dark
                    };
                    self.save_settings();
                    self.apply_widget_settings();
                } else if let Some(secs) =
                    other.strip_prefix("interval:").and_then(|s| s.parse().ok())
                {
                    self.set_interval(secs);
                }
            }
        }
        self.sync_menu();
        self.refresh_settings_form();
        self.render();
    }

    fn refresh(&self) {
        if let Some(b) = &self.backend {
            b.refresh_now();
        }
    }

    fn shutdown(mut self) {
        self.persist_cache();
        let _ = self.widget.hide();
        self.settings_win = None;
        if let Some(b) = self.backend.take() {
            b.stop(&self.rt);
        }
        // Drop the tray icon first so Windows removes it immediately.
        self.menu = None;
        self.tray = None;
    }
}

pub fn run() {
    if let Err(msg) = run_inner() {
        // No console in release builds, so leave a readable trace.
        if let Ok(paths) = Paths::system() {
            let _ = std::fs::create_dir_all(&paths.dir);
            let _ = std::fs::write(paths.dir.join("startup-error.txt"), msg);
        }
    }
}

fn run_inner() -> Result<(), String> {
    // Single instance: a second launch exits quietly.
    let instance =
        single_instance::SingleInstance::new(INSTANCE_NAME).map_err(|e| e.to_string())?;
    if !instance.is_single() {
        return Ok(());
    }

    let paths = Paths::system().map_err(|e| e.to_string())?;
    let (settings, outcome) = aum_store::load_settings(&paths);
    let (cache, _) = aum_store::load_cache(&paths);
    if settings.launch_at_login {
        // Keep the login entry pointing at the exe that is actually running
        // (it may have been moved or updated since it was registered).
        if let Ok(exe) = std::env::current_exe() {
            let _ = aum_platform::set_autostart(true, &exe);
        }
    }
    let notice = matches!(outcome, LoadOutcome::Recovered { .. })
        .then(|| "Settings were unreadable and were reset".to_owned());
    if outcome == LoadOutcome::Missing {
        let _ = aum_store::save_settings(&paths, &settings); // create the file with defaults
    }

    let selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into());
    // macOS: run as an accessory app (menu bar item, no Dock icon, no app menu). The app bundle
    // also sets LSUIElement; this covers running the bare binary.
    #[cfg(target_os = "macos")]
    let selector = {
        use slint::winit_030::winit::platform::macos::{
            ActivationPolicy, EventLoopBuilderExtMacOS,
        };
        let mut builder = slint::winit_030::winit::event_loop::EventLoop::with_user_event();
        builder.with_activation_policy(ActivationPolicy::Accessory);
        selector.with_winit_event_loop_builder(builder)
    };
    selector
        .select()
        .map_err(|e| format!("graphics backend: {e}"))?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    // Widget window (created hidden; shown by `show_widget`).
    let layout = Rc::new(RefCell::new(LayoutWatch::new(aum_platform::monitors())));
    let widget = Widget::new().map_err(|e| e.to_string())?;
    {
        let weak = widget.as_weak();
        widget.on_start_drag(move || {
            if let Some(w) = weak.upgrade() {
                w.window().with_winit_window(|ww| {
                    let _ = ww.drag_window();
                });
            }
        });
        widget.on_refresh_requested(|| with_app(|a| a.refresh()));
        widget.on_hide_requested(|| with_app(|a| a.set_widget_visible(false)));
        widget.on_toggle_compact_requested(|| with_app(|a| a.on_menu("widget:compact")));
        widget.window().on_close_requested(|| {
            // e.g. Alt+F4: hide instead of quitting, and remember the choice.
            let _ = slint::invoke_from_event_loop(|| with_app(|a| a.set_widget_visible(false)));
            slint::CloseRequestResponse::HideWindow
        });

        // Debounced position persistence. The handler must not touch the app
        // directly: moves can be triggered synchronously from inside app code.
        //
        // Moves made by the system while the monitor layout changes (a display plugged in or
        // removed, a resolution or scaling change) are not the user's choice and must not
        // overwrite the saved position, or the widget never returns to its spot.
        let pending: Rc<Cell<Option<WidgetPos>>> = Rc::new(Cell::new(None));
        let timer = Rc::new(Timer::default());
        widget.window().on_winit_window_event({
            let (pending, timer, layout) = (pending.clone(), timer.clone(), layout.clone());
            move |_, event| {
                use winit::event::WindowEvent as E;
                if matches!(
                    event,
                    E::Moved(_) | E::Resized(_) | E::ScaleFactorChanged { .. }
                ) {
                    let now = Instant::now();
                    // `monitors()` is empty where the platform module cannot list them (macOS);
                    // the watch then never reports a change and behaviour is unchanged.
                    let changed = layout.borrow_mut().observe(&aum_platform::monitors(), now);
                    if changed {
                        pending.set(None);
                        let _ = slint::invoke_from_event_loop(|| {
                            with_app(|a| a.on_display_change());
                        });
                    }
                    if let E::Moved(p) = event
                        && layout.borrow().is_user_move(now)
                    {
                        pending.set(Some(WidgetPos { x: p.x, y: p.y }));
                        let pending = pending.clone();
                        timer.start(TimerMode::SingleShot, SAVE_POS_DELAY, move || {
                            if let Some(pos) = pending.take() {
                                with_app(|a| a.save_position(pos));
                            }
                        });
                    }
                }
                EventResult::Propagate
            }
        });
    }

    let backend = Backend::start(&rt, &settings, cache);

    // Tray icon and menu.
    let initial_activity: HashMap<ProviderId, ActivityView> =
        if settings.claude_enabled && settings.activity.enabled {
            HashMap::from([(ProviderId::Claude, ActivityView::empty())])
        } else {
            HashMap::new()
        };
    let model = view::build_with_activity(
        &backend.collector.snapshot(),
        &initial_activity,
        Utc::now(),
        settings.poll_interval(),
    );
    let update_state = UpdateState::load(&paths);
    let update_ui = update_menu_text(&update_state, &settings, false);
    let (menu, menu_state) = build_menu(&model, notice.as_deref(), &settings, &update_ui);
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(&model.tooltip)
        .with_icon(make_icon(&model))
        .build()
        .map_err(|e| format!("tray: {e}"))?;

    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        let id = e.id.0.clone();
        let _ = slint::invoke_from_event_loop(move || with_app(|a| a.on_menu(&id)));
    }));

    let tick = Timer::default();
    tick.start(TimerMode::Repeated, TICK, || with_app(App::render));

    let show_at_start = settings.widget_visible;
    let mut notifier = notify::Engine::new();
    notifier.import(aum_store::load_notified(&paths));
    let app = App {
        rt,
        paths,
        settings,
        notice,
        backend: Some(backend),
        tray: Some(tray),
        menu: Some(menu_state),
        widget,
        last_widget_model: None,
        _tick: tick,
        retry: Timer::default(),
        settle: Timer::default(),
        display_poll: Timer::default(),
        update: update_state,
        update_busy: false,
        update_timer: Timer::default(),
        update_first: Timer::default(),
        prompt_timer: Timer::default(),
        prompt_win: None,
        layout,
        notifier,
        settings_win: None,
        trackers: HashMap::new(),
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    let _keep_instance_lock = instance;

    if show_at_start {
        let _ = slint::invoke_from_event_loop(|| with_app(|a| a.show_widget()));
    }
    let _ = slint::invoke_from_event_loop(|| {
        with_app(|a| {
            a.schedule_update_checks(false);
            // Ask the one-time question a little after start, not in the middle of the first paint.
            a.prompt_timer
                .start(TimerMode::SingleShot, Duration::from_secs(20), || {
                    with_app(App::maybe_prompt_for_updates)
                });
        })
    });

    slint::run_event_loop_until_quit().map_err(|e| e.to_string())?;

    if let Some(app) = APP.with(|cell| cell.borrow_mut().take()) {
        app.shutdown();
    }
    Ok(())
}
