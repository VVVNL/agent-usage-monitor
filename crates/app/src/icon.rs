//! Tray icon pixels, drawn in code so the icon can reflect usage live.
//! A vertical gauge: the fill height is the usage, the colour is the level. With two providers
//! (Claude Code and Codex) the icon holds two narrower gauges side by side, the first provider on the
//! left, each with its own fill, colour and staleness.
//! No text is rendered (illegible at 16-32 px); numbers live in the tooltip.

pub const SIZE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Critical,
    Unknown,
}

type Rgba = [u8; 4];

const FRAME: Rgba = [34, 34, 38, 255];
const EMPTY: Rgba = [78, 78, 86, 255];
const CLEAR: Rgba = [0, 0, 0, 0];

fn color(level: Level, stale: bool) -> Rgba {
    let c = match level {
        Level::Ok => [52, 199, 89, 255],
        Level::Warn => [255, 179, 0, 255],
        Level::Critical => [235, 64, 52, 255],
        Level::Unknown => EMPTY,
    };
    if stale && level != Level::Unknown {
        // Desaturate stale data toward grey so it never looks live.
        let g = ((c[0] as u16 + c[1] as u16 + c[2] as u16) / 3) as u8;
        return [
            (c[0] / 2) + g / 2,
            (c[1] / 2) + g / 2,
            (c[2] / 2) + g / 2,
            255,
        ];
    }
    c
}

/// A small dot in the top-right corner for session activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Badge {
    #[default]
    None,
    /// At least one session is working.
    Working,
    /// At least one session is waiting for the user.
    NeedsInput,
}

const BADGE_WORKING: Rgba = [10, 132, 255, 255];
const BADGE_ATTENTION: Rgba = [255, 159, 10, 255];

/// One provider's gauge: its usage, level and whether its data is stale or in error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gauge {
    /// `None` when there is no current value: the bar is drawn empty, never as 0% green.
    pub percent: Option<u8>,
    pub level: Level,
    pub stale: bool,
}

/// The icon for one gauge per provider (one gauge: the classic icon; two or more: the first two side by
/// side, left to right). Returns RGBA8 pixels, `SIZE` x `SIZE`.
pub fn render_gauges(gauges: &[Gauge], badge: Badge) -> Vec<u8> {
    match gauges {
        [] => render_badged(None, Level::Unknown, false, badge),
        [g] => render_badged(g.percent, g.level, g.stale, badge),
        [left, right, ..] => {
            let mut px = render_split(left, right);
            draw_badge(&mut px, badge);
            px
        }
    }
}

/// Returns RGBA8 pixels, `SIZE` x `SIZE`.
pub fn render(percent: Option<u8>, level: Level, stale: bool) -> Vec<u8> {
    render_badged(percent, level, stale, Badge::None)
}

/// Like [`render`], with an activity badge. The dot sits in the free margin right of the gauge, and
/// its coordinates are even so the 16 px downscale stays crisp.
pub fn render_badged(percent: Option<u8>, level: Level, stale: bool, badge: Badge) -> Vec<u8> {
    let mut px = render_gauge(percent, level, stale);
    draw_badge(&mut px, badge);
    px
}

fn draw_badge(px: &mut [u8], badge: Badge) {
    let color = match badge {
        Badge::None => return,
        Badge::Working => BADGE_WORKING,
        Badge::NeedsInput => BADGE_ATTENTION,
    };
    for y in 2..8i32 {
        for x in 24..30i32 {
            let (dx, dy) = (x as f32 + 0.5 - 27.0, y as f32 + 0.5 - 5.0);
            if dx * dx + dy * dy <= 9.5 {
                let i = ((y * SIZE as i32 + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&color);
            }
        }
    }
}

fn render_gauge(percent: Option<u8>, level: Level, stale: bool) -> Vec<u8> {
    let mut px = vec![0u8; (SIZE * SIZE * 4) as usize];
    let s = SIZE as i32;
    let (bar_x0, bar_x1, bar_y0, bar_y1) = (8, 24, 4, 28); // exclusive upper bounds; all even so a 2x downscale to 16 px stays crisp
    let bar_h = bar_y1 - bar_y0;
    // Round up so 1% is visible, then to an even height so the 16 px downscale has no half-pixel row.
    let filled = percent.map_or(0, |p| ((p.min(100) as i32) * bar_h + 99) / 100);
    let filled = (((filled + 1) / 2) * 2).min(bar_h);
    let fill = color(level, stale);

    for y in 0..s {
        for x in 0..s {
            // Rounded frame, corner radius 6.
            let (cx, cy) = (x.min(s - 1 - x), y.min(s - 1 - y));
            let in_frame = if cx < 6 && cy < 6 {
                let (dx, dy) = (6 - cx, 6 - cy);
                dx * dx + dy * dy <= 36
            } else {
                true
            };
            let c = if !in_frame {
                CLEAR
            } else if (bar_x0..bar_x1).contains(&x) && (bar_y0..bar_y1).contains(&y) {
                if bar_y1 - y <= filled { fill } else { EMPTY }
            } else {
                FRAME
            };
            let i = ((y * s + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    px
}

/// Is (x, y) inside the rounded frame (corner radius 6)?
fn in_frame(x: i32, y: i32) -> bool {
    let s = SIZE as i32;
    let (cx, cy) = (x.min(s - 1 - x), y.min(s - 1 - y));
    if cx < 6 && cy < 6 {
        let (dx, dy) = (6 - cx, 6 - cy);
        dx * dx + dy * dy <= 36
    } else {
        true
    }
}

/// Fill height in pixels for `percent` of a bar `bar_h` tall: rounded up so 1% is visible, then to an
/// even height so the 16 px downscale has no half-pixel row.
fn filled_height(percent: Option<u8>, bar_h: i32) -> i32 {
    let filled = percent.map_or(0, |p| ((i32::from(p.min(100))) * bar_h + 99) / 100);
    (((filled + 1) / 2) * 2).min(bar_h)
}

/// Two bars side by side in the frame. The bars start at y = 8 (not 4) so the activity badge in the
/// top-right corner never covers one; all coordinates are even so the 16 px downscale stays crisp.
fn render_split(left: &Gauge, right: &Gauge) -> Vec<u8> {
    const BARS: [(i32, i32); 2] = [(6, 14), (18, 26)]; // x ranges, exclusive upper bound
    const Y0: i32 = 8;
    const Y1: i32 = 28;
    let s = SIZE as i32;
    let bar_h = Y1 - Y0;
    let fills = [
        (
            filled_height(left.percent, bar_h),
            color(left.level, left.stale),
        ),
        (
            filled_height(right.percent, bar_h),
            color(right.level, right.stale),
        ),
    ];
    let mut px = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..s {
        for x in 0..s {
            let c = if !in_frame(x, y) {
                CLEAR
            } else if let Some(bar) = BARS.iter().position(|&(x0, x1)| (x0..x1).contains(&x)) {
                if (Y0..Y1).contains(&y) {
                    let (filled, fill) = fills[bar];
                    if Y1 - y <= filled { fill } else { EMPTY }
                } else {
                    FRAME
                }
            } else {
                FRAME
            };
            let i = ((y * s + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], x: u32, y: u32) -> Rgba {
        let i = ((y * SIZE + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn buffer_size_is_correct() {
        assert_eq!(
            render(Some(50), Level::Ok, false).len(),
            (SIZE * SIZE * 4) as usize
        );
    }

    #[test]
    fn corners_are_transparent_and_frame_is_opaque() {
        let px = render(Some(50), Level::Ok, false);
        assert_eq!(at(&px, 0, 0)[3], 0);
        assert_eq!(at(&px, 16, 1), FRAME);
    }

    #[test]
    fn fill_height_tracks_percent() {
        let px = render(Some(50), Level::Ok, false);
        assert_eq!(at(&px, 16, 27), color(Level::Ok, false), "bottom is filled");
        assert_eq!(at(&px, 16, 5), EMPTY, "top is empty at 50%");
        let full = render(Some(100), Level::Critical, false);
        assert_eq!(at(&full, 16, 4), color(Level::Critical, false));
        let none = render(Some(0), Level::Ok, false);
        assert_eq!(at(&none, 16, 27), EMPTY);
    }

    #[test]
    fn one_percent_is_still_visible() {
        let px = render(Some(1), Level::Ok, false);
        assert_eq!(at(&px, 16, 27), color(Level::Ok, false));
    }

    #[test]
    fn unknown_and_stale_look_different_from_live() {
        let live = render(Some(80), Level::Warn, false);
        let stale = render(Some(80), Level::Warn, true);
        assert_ne!(live, stale);
        let unknown = render(None, Level::Unknown, false);
        assert_eq!(at(&unknown, 16, 27), EMPTY);
    }

    #[test]
    fn out_of_range_percent_does_not_panic() {
        let _ = render(Some(255), Level::Critical, false);
    }

    #[test]
    fn badge_is_drawn_only_when_asked_and_does_not_touch_the_gauge() {
        let plain = render(Some(60), Level::Ok, false);
        assert_eq!(
            plain,
            render_badged(Some(60), Level::Ok, false, Badge::None)
        );
        let working = render_badged(Some(60), Level::Ok, false, Badge::Working);
        let attention = render_badged(Some(60), Level::Ok, false, Badge::NeedsInput);
        assert_eq!(at(&working, 27, 5), BADGE_WORKING);
        assert_eq!(at(&attention, 27, 5), BADGE_ATTENTION);
        assert_ne!(working, attention);
        // The gauge bar (x 8..24) is untouched by the badge (x 24..30).
        for y in 0..SIZE {
            for x in 0..24 {
                assert_eq!(at(&plain, x, y), at(&working, x, y), "({x},{y})");
            }
        }
        // And the dot is not a full square: its corner pixel is still frame colour.
        assert_eq!(at(&working, 24, 2), at(&plain, 24, 2));
    }

    // ----- two providers: one gauge each, side by side (left = first provider) -----

    fn g(percent: u8, level: Level, stale: bool) -> Gauge {
        Gauge {
            percent: Some(percent),
            level,
            stale,
        }
    }

    #[test]
    fn one_gauge_is_exactly_the_existing_single_icon() {
        for badge in [Badge::None, Badge::Working, Badge::NeedsInput] {
            assert_eq!(
                render_gauges(&[g(60, Level::Warn, false)], badge),
                render_badged(Some(60), Level::Warn, false, badge)
            );
        }
        assert_eq!(
            render_gauges(&[], Badge::None),
            render_badged(None, Level::Unknown, false, Badge::None),
            "no gauges looks like no data"
        );
    }

    #[test]
    fn two_gauges_are_two_separate_bars_with_their_own_fill_and_colour() {
        let px = render_gauges(
            &[g(80, Level::Warn, false), g(20, Level::Ok, false)],
            Badge::None,
        );
        assert_eq!(px.len(), (SIZE * SIZE * 4) as usize);
        // Bottom row of each bar: left is amber (80%), right is green (20%).
        assert_eq!(at(&px, 9, 27), color(Level::Warn, false));
        assert_eq!(at(&px, 22, 27), color(Level::Ok, false));
        // Higher up (about 50% of the bar): the 80% bar is still filled, the 20% bar is empty.
        assert_eq!(at(&px, 9, 18), color(Level::Warn, false));
        assert_eq!(at(&px, 22, 18), EMPTY);
        // The two bars are separated by frame colour, and the outer frame is intact.
        assert_eq!(at(&px, 16, 18), FRAME);
        assert_eq!(at(&px, 16, 1), FRAME);
        assert_eq!(at(&px, 0, 0)[3], 0, "corners stay transparent");
    }

    #[test]
    fn each_gauge_is_dimmed_only_for_its_own_staleness() {
        let both_live = render_gauges(
            &[g(50, Level::Ok, false), g(50, Level::Ok, false)],
            Badge::None,
        );
        let right_stale = render_gauges(
            &[g(50, Level::Ok, false), g(50, Level::Ok, true)],
            Badge::None,
        );
        assert_eq!(
            at(&both_live, 9, 27),
            at(&right_stale, 9, 27),
            "left unchanged"
        );
        assert_ne!(
            at(&both_live, 22, 27),
            at(&right_stale, 22, 27),
            "right dimmed"
        );
        assert_eq!(at(&right_stale, 22, 27), color(Level::Ok, true));
    }

    #[test]
    fn a_gauge_without_a_value_is_empty_not_zero_percent_green() {
        let none = Gauge {
            percent: None,
            level: Level::Unknown,
            stale: false,
        };
        let px = render_gauges(&[g(70, Level::Ok, false), none], Badge::None);
        assert_eq!(at(&px, 22, 27), EMPTY);
        assert_eq!(at(&px, 9, 27), color(Level::Ok, false));
    }

    #[test]
    fn one_percent_and_full_are_visible_in_each_gauge() {
        let px = render_gauges(
            &[g(1, Level::Ok, false), g(100, Level::Critical, false)],
            Badge::None,
        );
        assert_eq!(at(&px, 9, 27), color(Level::Ok, false), "1% still shows");
        assert_eq!(
            at(&px, 22, 8),
            color(Level::Critical, false),
            "100% reaches the top"
        );
    }

    #[test]
    fn the_badge_never_covers_a_gauge_in_split_mode() {
        let plain = render_gauges(
            &[
                g(100, Level::Critical, false),
                g(100, Level::Critical, false),
            ],
            Badge::None,
        );
        let badged = render_gauges(
            &[
                g(100, Level::Critical, false),
                g(100, Level::Critical, false),
            ],
            Badge::NeedsInput,
        );
        assert_eq!(at(&badged, 27, 5), BADGE_ATTENTION);
        // Every pixel the badge changed is outside both bars.
        for y in 0..SIZE {
            for x in 0..SIZE {
                if at(&plain, x, y) != at(&badged, x, y) {
                    let in_bar = (6..14).contains(&x) || (18..26).contains(&x);
                    assert!(
                        !(in_bar && (8..28).contains(&y)),
                        "badge touched a gauge at ({x},{y})"
                    );
                }
            }
        }
    }

    #[test]
    fn split_icon_stays_crisp_at_16_px() {
        // The tray often shows the 32 px image at 16 px (a 2x box filter). Every 2x2 block inside the
        // frame must therefore be one colour, or the bars blur.
        let px = render_gauges(
            &[g(37, Level::Warn, false), g(63, Level::Ok, true)],
            Badge::Working,
        );
        for by in 3..14 {
            for bx in 3..13 {
                let (x, y) = (bx * 2, by * 2);
                if x >= 24 && y < 8 {
                    continue; // the badge's round edge is deliberately not block-aligned
                }
                let c = at(&px, x, y);
                for (dx, dy) in [(1, 0), (0, 1), (1, 1)] {
                    assert_eq!(at(&px, x + dx, y + dy), c, "block ({x},{y})");
                }
            }
        }
    }
}
