//! Tray icon pixels, drawn in code so the icon can reflect usage live.
//! A vertical gauge: the fill height is the usage, the colour is the level.
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

/// Returns RGBA8 pixels, `SIZE` x `SIZE`.
pub fn render(percent: Option<u8>, level: Level, stale: bool) -> Vec<u8> {
    render_badged(percent, level, stale, Badge::None)
}

/// Like [`render`], with an activity badge. The dot sits in the free margin right of the gauge, and
/// its coordinates are even so the 16 px downscale stays crisp.
pub fn render_badged(percent: Option<u8>, level: Level, stale: bool, badge: Badge) -> Vec<u8> {
    let mut px = render_gauge(percent, level, stale);
    let color = match badge {
        Badge::None => return px,
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
    px
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
}
