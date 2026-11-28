//! Entry point. With `shell` enabled, open one Silvestris window with a live
//! PTY; without it, print the linked library version for the headless M0
//! smoke test.

// The shell build is a GUI application: a console-subsystem binary makes
// Windows create a second, empty console window when it is launched from
// Explorer. The headless build keeps its console so its version line is
// visible.
#![cfg_attr(
    all(feature = "shell", target_os = "windows"),
    windows_subsystem = "windows"
)]

fn main() {
    #[cfg(feature = "shell")]
    {
        pain::app::run();
    }
    #[cfg(not(feature = "shell"))]
    {
        println!(
            "pain {}: headless libghostty-vt spike (M0); build with --features shell for the M1 window",
            pain_terminal_core::version()
        );
    }
}
