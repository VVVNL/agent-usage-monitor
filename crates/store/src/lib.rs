//! Local persistence: plain JSON files, written atomically.
//!
//! - settings.json : user preferences
//! - cache.json    : last-known provider states (shown stale after restart)
//!
//! Nothing secret is ever stored here.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use aum_core::ProviderState;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialisation error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not determine a config directory")]
    NoDirs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// Top-left corner of the widget in physical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetPos {
    pub x: i32,
    pub y: i32,
}

/// Per-provider notification preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyConfig {
    pub enabled: bool,
    /// First alert level, percent used (default 75).
    pub warn_percent: u8,
    /// Second alert level, percent used (default 90).
    pub critical_percent: u8,
    pub exhausted: bool,
    pub reset: bool,
    /// Login and provider errors.
    pub errors: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            warn_percent: 75,
            critical_percent: 90,
            exhausted: true,
            reset: true,
            errors: true,
        }
    }
}

impl NotifyConfig {
    /// Thresholds forced into a sane, ordered range (hand-edited files cannot break alerting).
    pub fn levels(&self) -> (u8, u8) {
        let warn = self.warn_percent.clamp(1, 98);
        let critical = self.critical_percent.clamp(warn + 1, 99);
        (warn, critical)
    }
}

/// Session activity (running / waiting / finished sessions) preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivitySettings {
    pub enabled: bool,
    /// How long "finished N min ago" stays visible, in minutes.
    pub finished_minutes: u32,
    pub alert_needs_input: bool,
    pub alert_finished: bool,
    /// Only alert for a finished session that ran at least this long (seconds).
    pub alert_min_secs: u32,
    /// Windows only: WSL distributions to look inside for sessions (opt-in, running ones only).
    pub wsl_distros: Vec<String>,
    /// Explicit path to `claude` inside WSL; empty means automatic.
    pub wsl_claude_path: Option<String>,
}

impl Default for ActivitySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            finished_minutes: 10,
            alert_needs_input: false,
            alert_finished: false,
            alert_min_secs: 60,
            wsl_distros: Vec::new(),
            wsl_claude_path: None,
        }
    }
}

impl ActivitySettings {
    pub fn finished_ttl(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.finished_minutes.clamp(1, 120)) * 60)
    }
    pub fn min_run(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.alert_min_secs.min(3600)))
    }
}

/// User preferences. Missing fields take defaults and unknown fields are
/// ignored, so settings files survive upgrades and downgrades.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub claude_enabled: bool,
    /// Optional explicit path to the Claude Code CLI.
    pub claude_cli_path: Option<String>,
    pub poll_interval_secs: u64,
    pub widget_visible: bool,
    pub widget_always_on_top: bool,
    pub widget_compact: bool,
    /// Whole-window opacity, 30..=100 (clamped on use).
    pub widget_opacity_percent: u8,
    pub widget_theme: Theme,
    /// Mouse clicks pass through the widget. Reversible from the tray menu.
    pub widget_click_through: bool,
    pub widget_pos: Option<WidgetPos>,
    pub claude_notify: NotifyConfig,
    /// Codex CLI is off by default: it only works if Codex is installed and signed in.
    pub codex_enabled: bool,
    pub codex_cli_path: Option<String>,
    pub codex_notify: NotifyConfig,
    pub activity: ActivitySettings,
    pub launch_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            claude_enabled: true,
            claude_cli_path: None,
            poll_interval_secs: 300,
            widget_visible: true,
            widget_always_on_top: true,
            widget_compact: false,
            widget_opacity_percent: 95,
            widget_theme: Theme::Dark,
            widget_click_through: false,
            widget_pos: None,
            claude_notify: NotifyConfig::default(),
            codex_enabled: false,
            codex_cli_path: None,
            codex_notify: NotifyConfig::default(),
            activity: ActivitySettings::default(),
            launch_at_login: false,
        }
    }
}

impl Settings {
    /// Poll interval with the floor applied (corrupt or hand-edited values
    /// can never cause rapid polling).
    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.poll_interval_secs.clamp(60, 86_400))
    }

    /// Opacity with the floor applied, so the widget can never be made invisible.
    pub fn opacity_percent(&self) -> u8 {
        self.widget_opacity_percent.clamp(30, 100)
    }
}

/// A rectangle in physical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    fn overlap(&self, other: &Rect) -> (i32, i32) {
        let ow = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let oh = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        (ow.max(0), oh.max(0))
    }
}

/// Minimum visible part of the widget (px) for a saved position to be kept.
const MIN_VISIBLE: (i32, i32) = (48, 32);
const MARGIN: i32 = 24;

/// Where to place the widget. A saved position is kept only if enough of the
/// window is visible on some monitor (a monitor may have been unplugged or the
/// resolution changed); otherwise the widget goes to the top-right corner of the
/// first monitor (the primary one). `monitors` must not be empty.
pub fn place_widget(saved: Option<WidgetPos>, size: (i32, i32), monitors: &[Rect]) -> WidgetPos {
    let Some(primary) = monitors.first() else {
        return saved.unwrap_or(WidgetPos {
            x: MARGIN,
            y: MARGIN,
        });
    };
    if let Some(p) = saved {
        let win = Rect::new(p.x, p.y, size.0, size.1);
        let visible = monitors.iter().any(|m| {
            let (w, h) = win.overlap(m);
            w >= MIN_VISIBLE.0.min(size.0) && h >= MIN_VISIBLE.1.min(size.1)
        });
        if visible {
            return p;
        }
    }
    WidgetPos {
        x: primary.x + primary.w - size.0 - MARGIN,
        y: primary.y + MARGIN,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    Loaded,
    Missing,
    /// File was unreadable; it was moved aside and defaults are in use.
    Recovered {
        backup: PathBuf,
    },
}

/// Locations of the app's files.
#[derive(Debug, Clone)]
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn system() -> Result<Self, StoreError> {
        let dirs = directories::ProjectDirs::from("", "", "AgentUsageMonitor")
            .ok_or(StoreError::NoDirs)?;
        Ok(Self {
            dir: dirs.config_dir().to_path_buf(),
        })
    }
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn settings(&self) -> PathBuf {
        self.dir.join("settings.json")
    }
    pub fn cache(&self) -> PathBuf {
        self.dir.join("cache.json")
    }
    pub fn notified(&self) -> PathBuf {
        self.dir.join("notified.json")
    }
}

/// Write via a temp file in the same directory, then rename, so a crash can
/// never leave a half-written file.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let dir = path.parent().ok_or(StoreError::NoDirs)?;
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(value)?)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Read JSON, falling back to `T::default()`. A corrupt file is renamed to
/// `*.corrupt` (so it can be inspected) rather than deleted or crashed on.
pub fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> (T, LoadOutcome) {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (T::default(), LoadOutcome::Missing);
        }
        Err(_) => return (T::default(), quarantine(path)),
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => (v, LoadOutcome::Loaded),
        Err(_) => (T::default(), quarantine(path)),
    }
}

fn quarantine(path: &Path) -> LoadOutcome {
    let backup = path.with_extension("json.corrupt");
    let _ = fs::rename(path, &backup);
    LoadOutcome::Recovered { backup }
}

pub fn load_settings(paths: &Paths) -> (Settings, LoadOutcome) {
    read_json_or_default(&paths.settings())
}

pub fn save_settings(paths: &Paths, s: &Settings) -> Result<(), StoreError> {
    write_json_atomic(&paths.settings(), s)
}

pub fn load_cache(paths: &Paths) -> (Vec<ProviderState>, LoadOutcome) {
    read_json_or_default(&paths.cache())
}

/// Keys of notifications already shown, so a restart never repeats an alert.
pub fn load_notified(paths: &Paths) -> Vec<String> {
    read_json_or_default::<Vec<String>>(&paths.notified()).0
}

pub fn save_notified(paths: &Paths, keys: &[String]) -> Result<(), StoreError> {
    write_json_atomic(&paths.notified(), &keys)
}

pub fn save_cache(paths: &Paths, states: &[ProviderState]) -> Result<(), StoreError> {
    write_json_atomic(&paths.cache(), &states)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aum_core::{ProviderId, ProviderSnapshot};
    use chrono::Utc;

    fn paths() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(d.path().join("cfg"));
        (d, p)
    }

    #[test]
    fn missing_file_gives_defaults() {
        let (_d, p) = paths();
        let (s, o) = load_settings(&p);
        assert_eq!(s, Settings::default());
        assert_eq!(o, LoadOutcome::Missing);
    }

    #[test]
    fn settings_round_trip() {
        let (_d, p) = paths();
        let s = Settings {
            claude_enabled: false,
            claude_cli_path: Some("C:\\tools\\claude.exe".into()),
            poll_interval_secs: 600,
            widget_visible: false,
            widget_compact: true,
            widget_opacity_percent: 60,
            widget_theme: Theme::Light,
            widget_pos: Some(WidgetPos { x: -1500, y: 40 }),
            ..Settings::default()
        };
        save_settings(&p, &s).unwrap();
        let (loaded, o) = load_settings(&p);
        assert_eq!(loaded, s);
        assert_eq!(o, LoadOutcome::Loaded);
        assert!(
            !p.dir.join("settings.json.tmp").exists(),
            "temp file cleaned up"
        );
    }

    #[test]
    fn partial_and_future_settings_files_load() {
        let (_d, p) = paths();
        fs::create_dir_all(&p.dir).unwrap();
        fs::write(
            p.settings(),
            r#"{"poll_interval_secs": 900, "some_future_field": true}"#,
        )
        .unwrap();
        let (s, o) = load_settings(&p);
        assert_eq!(s.poll_interval_secs, 900);
        assert!(s.claude_enabled);
        assert_eq!(o, LoadOutcome::Loaded);
    }

    #[test]
    fn corrupt_settings_are_quarantined_not_fatal() {
        let (_d, p) = paths();
        fs::create_dir_all(&p.dir).unwrap();
        fs::write(p.settings(), "{ not json").unwrap();
        let (s, o) = load_settings(&p);
        assert_eq!(s, Settings::default());
        match o {
            LoadOutcome::Recovered { backup } => {
                assert!(backup.exists());
                assert!(!p.settings().exists());
            }
            other => panic!("unexpected {other:?}"),
        }
        // And a fresh save works afterwards.
        save_settings(&p, &Settings::default()).unwrap();
    }

    #[test]
    fn truncated_and_wrong_type_files_are_recovered() {
        let (_d, p) = paths();
        fs::create_dir_all(&p.dir).unwrap();
        fs::write(p.cache(), r#"[{"provider":"Claude","stat"#).unwrap();
        assert!(matches!(load_cache(&p).1, LoadOutcome::Recovered { .. }));
        fs::write(p.settings(), r#"{"poll_interval_secs": "soon"}"#).unwrap();
        assert!(matches!(load_settings(&p).1, LoadOutcome::Recovered { .. }));
    }

    #[test]
    fn poll_interval_is_clamped() {
        let s = Settings {
            poll_interval_secs: 1,
            ..Settings::default()
        };
        assert_eq!(s.poll_interval().as_secs(), 60);
        let s = Settings {
            poll_interval_secs: u64::MAX,
            ..Settings::default()
        };
        assert_eq!(s.poll_interval().as_secs(), 86_400);
    }

    #[test]
    fn cache_round_trip_keeps_snapshot() {
        let (_d, p) = paths();
        let mut st = ProviderState::new(ProviderId::Claude);
        st.snapshot = Some(ProviderSnapshot {
            provider: ProviderId::Claude,
            windows: vec![],
            plan: Some("team".into()),
            fetched_at: Utc::now(),
            unrecognized: vec![],
        });
        save_cache(&p, &[st.clone()]).unwrap();
        let (loaded, o) = load_cache(&p);
        assert_eq!(o, LoadOutcome::Loaded);
        assert_eq!(loaded, vec![st]);
    }

    const SIZE: (i32, i32) = (240, 150);
    fn primary() -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }
    fn left() -> Rect {
        Rect::new(-1920, 0, 1920, 1080)
    }

    #[test]
    fn new_install_goes_top_right_of_primary() {
        let p = place_widget(None, SIZE, &[primary()]);
        assert_eq!(
            p,
            WidgetPos {
                x: 1920 - 240 - 24,
                y: 24
            }
        );
    }

    #[test]
    fn saved_position_on_second_monitor_is_kept() {
        let saved = WidgetPos { x: -1500, y: 200 }; // negative coords: monitor left of primary
        assert_eq!(place_widget(Some(saved), SIZE, &[primary(), left()]), saved);
    }

    #[test]
    fn unplugged_monitor_falls_back_to_primary() {
        let saved = WidgetPos { x: -1500, y: 200 };
        let p = place_widget(Some(saved), SIZE, &[primary()]);
        assert_eq!(
            p,
            WidgetPos {
                x: 1920 - 240 - 24,
                y: 24
            }
        );
    }

    #[test]
    fn resolution_change_that_strands_the_widget_is_corrected() {
        let saved = WidgetPos { x: 1800, y: 1000 }; // fine on 1920x1080
        let smaller = Rect::new(0, 0, 1280, 720);
        let p = place_widget(Some(saved), SIZE, &[smaller]);
        assert_eq!(
            p,
            WidgetPos {
                x: 1280 - 240 - 24,
                y: 24
            }
        );
    }

    #[test]
    fn partially_offscreen_but_grabbable_is_kept() {
        let saved = WidgetPos {
            x: 1920 - 100,
            y: 500,
        }; // 100 px visible
        assert_eq!(place_widget(Some(saved), SIZE, &[primary()]), saved);
        let barely = WidgetPos {
            x: 1920 - 10,
            y: 500,
        }; // 10 px visible: not grabbable
        assert_ne!(place_widget(Some(barely), SIZE, &[primary()]), barely);
    }

    #[test]
    fn opacity_cannot_make_the_widget_invisible() {
        let s = Settings {
            widget_opacity_percent: 0,
            ..Settings::default()
        };
        assert_eq!(s.opacity_percent(), 30);
        let s = Settings {
            widget_opacity_percent: 250,
            ..Settings::default()
        };
        assert_eq!(s.opacity_percent(), 100);
    }

    #[test]
    fn old_settings_file_gets_widget_defaults() {
        let (_d, p) = paths();
        fs::create_dir_all(&p.dir).unwrap();
        fs::write(
            p.settings(),
            r#"{"claude_enabled": true, "poll_interval_secs": 900}"#,
        )
        .unwrap();
        let (s, o) = load_settings(&p);
        assert_eq!(o, LoadOutcome::Loaded);
        assert!(s.widget_visible && s.widget_always_on_top && !s.widget_click_through);
        assert_eq!(s.widget_pos, None);
    }

    #[test]
    fn notify_levels_are_forced_into_order() {
        let c = NotifyConfig {
            warn_percent: 95,
            critical_percent: 10,
            ..NotifyConfig::default()
        };
        let (w, k) = c.levels();
        assert!(w < k && k <= 99 && w >= 1, "got {w},{k}");
        assert_eq!(NotifyConfig::default().levels(), (75, 90));
        let wild = NotifyConfig {
            warn_percent: 0,
            critical_percent: 255,
            ..NotifyConfig::default()
        };
        assert_eq!(wild.levels(), (1, 99));
    }

    #[test]
    fn notified_set_round_trips_and_survives_corruption() {
        let (_d, p) = paths();
        assert!(load_notified(&p).is_empty());
        save_notified(&p, &["a".into(), "b".into()]).unwrap();
        assert_eq!(load_notified(&p), vec!["a".to_owned(), "b".to_owned()]);
        fs::write(p.notified(), "garbage").unwrap();
        assert!(load_notified(&p).is_empty());
    }

    #[test]
    fn new_notification_settings_default_on_old_files() {
        let (_d, p) = paths();
        fs::create_dir_all(&p.dir).unwrap();
        fs::write(p.settings(), r#"{"poll_interval_secs": 300}"#).unwrap();
        let (s, _) = load_settings(&p);
        assert_eq!(s.claude_notify, NotifyConfig::default());
        assert!(!s.launch_at_login);
    }

    #[test]
    fn codex_is_off_by_default_and_old_files_still_load() {
        let (_d, p) = paths();
        fs::create_dir_all(&p.dir).unwrap();
        fs::write(
            p.settings(),
            r#"{"claude_enabled": true, "poll_interval_secs": 300}"#,
        )
        .unwrap();
        let (s, o) = load_settings(&p);
        assert_eq!(o, LoadOutcome::Loaded);
        assert!(!s.codex_enabled);
        assert_eq!(s.codex_cli_path, None);
        assert_eq!(s.codex_notify, NotifyConfig::default());
    }

    #[test]
    fn activity_defaults_are_quiet_and_local() {
        let a = ActivitySettings::default();
        assert!(a.enabled);
        assert!(
            !a.alert_needs_input && !a.alert_finished,
            "alerts are opt-in"
        );
        assert!(a.wsl_distros.is_empty(), "the WSL bridge is opt-in");
        assert_eq!(a.finished_ttl().as_secs(), 600);
        assert_eq!(a.min_run().as_secs(), 60);
    }

    #[test]
    fn activity_values_are_clamped_and_old_files_load() {
        let a = ActivitySettings {
            finished_minutes: 0,
            alert_min_secs: 999_999,
            ..ActivitySettings::default()
        };
        assert_eq!(a.finished_ttl().as_secs(), 60);
        assert_eq!(a.min_run().as_secs(), 3600);
        let big = ActivitySettings {
            finished_minutes: 100_000,
            ..ActivitySettings::default()
        };
        assert_eq!(big.finished_ttl().as_secs(), 7200);

        let (_d, p) = paths();
        fs::create_dir_all(&p.dir).unwrap();
        fs::write(
            p.settings(),
            r#"{"poll_interval_secs": 300, "activity": {"wsl_distros": ["Ubuntu"]}}"#,
        )
        .unwrap();
        let (s, o) = load_settings(&p);
        assert_eq!(o, LoadOutcome::Loaded);
        assert_eq!(s.activity.wsl_distros, vec!["Ubuntu".to_owned()]);
        assert!(
            s.activity.enabled,
            "fields missing from the file take defaults"
        );
    }
}
