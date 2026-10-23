//! Extension traits to suppress console windows on Windows.
//!
//! On Windows, spawned child processes open a visible console window by
//! default.  Call `.no_window()` before `.spawn()` or `.output()` to set
//! the `CREATE_NO_WINDOW` creation flag and prevent this.
//!
//! On non-Windows platforms the methods are no-ops.

use command_group::{AsyncCommandGroup, AsyncGroupChild};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Adds a `.no_window()` builder method that suppresses the console window
/// on Windows.  No-op on other platforms.
pub trait NoWindowExt {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindowExt for std::process::Command {
    #[cfg(windows)]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(windows))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

impl NoWindowExt for tokio::process::Command {
    #[cfg(windows)]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(windows))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

/// Adds a `.group_spawn_no_window()` helper for command-group spawns that
/// suppresses the console window on Windows and terminates the entire Windows
/// job when its owner is dropped. Other platforms retain their group behavior.
pub trait GroupSpawnNoWindowExt {
    fn group_spawn_no_window(&mut self) -> std::io::Result<AsyncGroupChild>;
}

impl GroupSpawnNoWindowExt for tokio::process::Command {
    fn group_spawn_no_window(&mut self) -> std::io::Result<AsyncGroupChild> {
        let mut group = self.group();
        #[cfg(windows)]
        // Tokio's command-level kill_on_drop only terminates the launcher
        // (often cmd.exe for a CLI .cmd shim). The job must own cleanup of its
        // descendants, including when discovery is cancelled or the host exits.
        group.kill_on_drop(true).creation_flags(CREATE_NO_WINDOW);
        group.spawn()
    }
}
