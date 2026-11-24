//! `tessel mcp` without `--headless` connects an agent to a running window
//! over a unix socket. Where there are none (Windows) it says so.

use std::ffi::OsString;

pub fn main(_args: impl IntoIterator<Item = OsString>) -> std::process::ExitCode {
    eprintln!(
        "tessel mcp: connecting to a running window is not available on this system yet.\n\
         Use `tessel mcp --headless --root DIR` to work on design files without a window."
    );
    std::process::ExitCode::from(2)
}
