//! Telling "a monitor was plugged in or removed" apart from "the user moved the widget".
//!
//! When the monitor layout changes, Windows moves the widget on its own (off a removed monitor,
//! back onto a returning one, clamped into a smaller work area) and the window reports each of
//! those moves like any other. Saving them overwrote the position the user had chosen, so after
//! the external display came back the widget stayed where the laptop screen had forced it. This
//! type notices the layout change and tells the caller to ignore the moves that follow it.
//! Pure logic, no GUI types.

use std::time::{Duration, Instant};

use aum_store::Rect;

/// How long after a layout change moves are treated as the system's, not the user's. Windows
/// finishes relocating windows and re-applying DPI within a second or so.
pub const SETTLE: Duration = Duration::from_millis(2500);

#[derive(Debug, Default)]
pub struct LayoutWatch {
    known: Vec<Rect>,
    quiet_until: Option<Instant>,
}

impl LayoutWatch {
    pub fn new(known: Vec<Rect>) -> Self {
        Self {
            known,
            quiet_until: None,
        }
    }

    /// Feed the current layout. Returns `true` when it differs from the last one seen, which
    /// also starts (or extends) the settling period. An empty list means "cannot tell" and is
    /// never a change.
    pub fn observe(&mut self, current: &[Rect], at: Instant) -> bool {
        if current.is_empty() || current == self.known.as_slice() {
            return false;
        }
        self.known = current.to_vec();
        self.quiet_until = Some(at + SETTLE);
        true
    }

    /// Is a window move at `at` the user's doing (as opposed to the system rearranging windows)?
    pub fn is_user_move(&self, at: Instant) -> bool {
        self.quiet_until.is_none_or(|until| at >= until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aum_store::{WidgetPos, place_widget};

    const LAPTOP: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1152,
    };
    const EXTERNAL: Rect = Rect {
        x: 1920,
        y: 0,
        w: 2560,
        h: 1392,
    };

    #[test]
    fn same_layout_is_not_a_change() {
        let t = Instant::now();
        let mut w = LayoutWatch::new(vec![LAPTOP]);
        assert!(!w.observe(&[LAPTOP], t));
        assert!(w.is_user_move(t));
    }

    #[test]
    fn plugging_in_a_monitor_is_a_change_and_silences_moves() {
        let t = Instant::now();
        let mut w = LayoutWatch::new(vec![LAPTOP]);
        assert!(w.observe(&[LAPTOP, EXTERNAL], t));
        assert!(!w.is_user_move(t + Duration::from_millis(100)));
        assert!(!w.is_user_move(t + SETTLE - Duration::from_millis(1)));
        assert!(
            w.is_user_move(t + SETTLE),
            "the user can move it again afterwards"
        );
        assert!(
            !w.observe(&[LAPTOP, EXTERNAL], t),
            "seen once is not a change again"
        );
    }

    #[test]
    fn a_resolution_change_counts() {
        let t = Instant::now();
        let mut w = LayoutWatch::new(vec![LAPTOP]);
        let smaller = Rect::new(0, 0, 1194, 752);
        assert!(w.observe(&[smaller], t));
    }

    #[test]
    fn a_second_change_while_settling_extends_the_quiet_period() {
        let t = Instant::now();
        let mut w = LayoutWatch::new(vec![LAPTOP]);
        assert!(w.observe(&[LAPTOP, EXTERNAL], t));
        let later = t + Duration::from_secs(2);
        assert!(w.observe(&[LAPTOP], later));
        assert!(!w.is_user_move(later + Duration::from_secs(2)));
        assert!(w.is_user_move(later + SETTLE));
    }

    #[test]
    fn an_unknown_layout_is_ignored() {
        let t = Instant::now();
        let mut w = LayoutWatch::new(vec![LAPTOP]);
        assert!(!w.observe(&[], t));
        assert!(w.is_user_move(t));
    }

    /// The reported bug, end to end with the shell's rules: the widget lives on the external
    /// monitor, the monitor is unplugged (the system moves the window), then plugged back in.
    #[test]
    fn widget_returns_to_the_users_spot_after_unplug_and_replug() {
        let size = (250, 212);
        let mut t = Instant::now();
        let mut watch = LayoutWatch::new(vec![LAPTOP, EXTERNAL]);
        // What the settings file holds: the user dragged the widget onto the external monitor.
        let mut saved = Some(WidgetPos { x: 4000, y: 100 });

        // Unplug: the layout changes first, then the system moves the window to the laptop.
        t += Duration::from_secs(10);
        assert!(watch.observe(&[LAPTOP], t));
        let shown = place_widget(saved, size, &[LAPTOP]); // what the shell does on a layout change
        assert!(LAPTOP.x <= shown.x && shown.x + size.0 <= LAPTOP.x + LAPTOP.w);
        let forced_by_system = WidgetPos { x: 1650, y: 100 };
        if watch.is_user_move(t + Duration::from_millis(200)) {
            saved = Some(forced_by_system); // the old behaviour: this overwrote the user's spot
        }
        assert_eq!(
            saved,
            Some(WidgetPos { x: 4000, y: 100 }),
            "forced move is not saved"
        );

        // Plug back in, well after the first change has settled.
        t += Duration::from_secs(60);
        assert!(watch.observe(&[LAPTOP, EXTERNAL], t));
        let shown = place_widget(saved, size, &[LAPTOP, EXTERNAL]);
        assert_eq!(shown, WidgetPos { x: 4000, y: 100 });

        // A genuine drag after things settle is still saved.
        t += Duration::from_secs(10);
        assert!(watch.is_user_move(t));
    }

    #[test]
    fn saved_spot_on_a_missing_monitor_falls_back_without_forgetting_it() {
        let saved = Some(WidgetPos { x: 4000, y: 100 });
        let shown = place_widget(saved, (250, 212), &[LAPTOP]);
        assert_ne!(Some(shown), saved, "an unreachable spot is not used");
        // `place_widget` is pure: the saved value the caller keeps is untouched.
        assert_eq!(saved, Some(WidgetPos { x: 4000, y: 100 }));
    }
}
