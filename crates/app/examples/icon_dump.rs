//! Writes raw RGBA icon renders for visual inspection: icon_dump <out_dir>
use aum_app::icon::{Level, SIZE, render};
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
    println!("{SIZE}");
}
