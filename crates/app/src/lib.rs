//! GUI-independent parts of the tray app: what to show, and the icon pixels.
//! Everything here is unit-tested on any OS; only `main.rs` touches Windows.

pub mod activity;
pub mod display;
pub mod form;
pub mod icon;
pub mod instance;
pub mod notify;
pub mod view;
pub mod widget;
