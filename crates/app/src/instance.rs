//! The key that makes a second launch of the app exit quietly.
//!
//! The `single-instance` crate reads its name differently per OS: on Windows it is the name of a
//! mutex, but on macOS it is **a file path** that is opened or created and locked. A bare name like
//! "AgentUsageMonitor-single-instance" therefore means "a file in the current directory", and an app
//! started from Finder or Launch Services has `/` as its current directory, which is not writable:
//! `SingleInstance::new` failed with "file open or create error" and the app quit before showing
//! anything (found by running the released DMG on a macOS runner, see
//! `.github/workflows/macos-smoke.yml`). On macOS the key is now an absolute path in the user's
//! temporary directory.

use std::path::Path;

pub const NAME: &str = "AgentUsageMonitor-single-instance";

/// The string to hand to `SingleInstance::new`. `temp_dir` is only used on macOS.
pub fn key(macos: bool, temp_dir: &Path) -> String {
    if macos {
        temp_dir
            .join("io.github.agent-usage-monitor.lock")
            .to_string_lossy()
            .into_owned()
    } else {
        NAME.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_key_is_the_plain_mutex_name() {
        assert_eq!(key(false, Path::new("/ignored")), NAME);
    }

    #[test]
    fn macos_key_is_an_absolute_path_inside_the_temp_dir() {
        let tmp = Path::new("/var/folders/ab/xyz/T");
        let k = key(true, tmp);
        assert!(Path::new(&k).is_absolute(), "{k}");
        assert!(Path::new(&k).starts_with(tmp));
        assert!(k.ends_with("io.github.agent-usage-monitor.lock"));
    }

    #[test]
    fn macos_key_is_not_a_bare_relative_name() {
        // The original bug: a relative name resolves against the current directory, which is `/`
        // when the app is started from Finder.
        assert_ne!(key(true, Path::new("/tmp")), NAME);
        assert!(!Path::new(&key(true, Path::new("/tmp"))).is_relative());
    }
}
