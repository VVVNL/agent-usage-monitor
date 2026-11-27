//! Writes raw RGBA icon renders for visual inspection: icon_dump <out_dir>
use aum_app::icon::{Badge, Gauge, Level, SIZE, render, render_gauges};
fn main() {
    let dir = std::env::args().nth(1).expect("out dir");
    let cases: [(&str, Option<u8>, Level, bool); 7] = [
        ("ok_35", Some(35), Level::Ok, false),
        ("warn_80", Some(80), Level::Warn, false),
        ("crit_95", Some(95), Level::Critical, false),
        ("full_100", Some(100), Level::Critical, false),
        ("low_1", Some(1), Level::Ok, false),
        ("stale_35", Some(35), Level::Ok, true),
        ("unknown", None, Level::Unknown, false),
    ];
    for (name, p, l, s) in cases {
        std::fs::write(format!("{dir}/{name}.rgba"), render(p, l, s)).unwrap();
    }
    // Two providers side by side (Claude Code left, Codex right).
    let g = |p: u8, l: Level, stale: bool| Gauge {
        percent: Some(p),
        level: l,
        stale,
    };
    let split: [(&str, [Gauge; 2], Badge); 5] = [
        (
            "split_82_7",
            [g(82, Level::Warn, false), g(7, Level::Ok, false)],
            Badge::None,
        ),
        (
            "split_35_95",
            [g(35, Level::Ok, false), g(95, Level::Critical, false)],
            Badge::None,
        ),
        (
            "split_right_stale",
            [g(50, Level::Ok, false), g(50, Level::Ok, true)],
            Badge::None,
        ),
        (
            "split_working",
            [g(60, Level::Ok, false), g(20, Level::Ok, false)],
            Badge::Working,
        ),
        (
            "split_needs_input",
            [
                g(100, Level::Critical, false),
                g(100, Level::Critical, false),
            ],
            Badge::NeedsInput,
        ),
    ];
    for (name, gauges, badge) in split {
        std::fs::write(format!("{dir}/{name}.rgba"), render_gauges(&gauges, badge)).unwrap();
    }
    println!("{SIZE}");
}
