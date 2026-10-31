//! `tessel [FILE]`: open the app, optionally with a document.
//! `tessel mcp [OPTIONS]`: the stdio bridge agents start (§6.1).
//! `tessel mcp --headless --root DIR`: the agent tools on files, no window (§6.7).
//! `tessel render FILE`: a design file to PNG, JPEG or SVG, no window (§6.7).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();

    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|a| a == "mcp") {
        let rest: Vec<_> = args.skip(1).collect();
        if rest.iter().any(|a| a == "--headless") {
            return headless::cli::mcp_main(rest);
        }
        return app::mcp_bridge::main(rest);
    }
    if args.peek().is_some_and(|a| a == "render") {
        return headless::cli::render_main(args.skip(1));
    }

    let mut file: Option<PathBuf> = None;
    for arg in args {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("usage: tessel [FILE.tessel.json]\n       tessel mcp --help\n       tessel render --help");
                return ExitCode::SUCCESS;
            }
            Some("-V" | "--version") => {
                println!("tessel {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            _ if file.is_none() => file = Some(PathBuf::from(arg)),
            _ => {
                log::error!("unexpected argument {arg:?}; usage: tessel [FILE]");
                return ExitCode::from(2);
            }
        }
    }

    match app::app::run(file) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
