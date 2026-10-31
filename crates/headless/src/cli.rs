//! `tessel render …` and `tessel mcp --headless …`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::gpu::Gpu;
use crate::render::{load, render, Format, RenderOptions};
use crate::workspace::Workspace;

pub const RENDER_USAGE: &str = "\
usage: tessel render FILE.tessel.json [-o OUT] [--node ID] [--format png|jpeg|svg]
                     [--scale N] [--background #rrggbb[aa]] [--quality 1-100]
                     [--layers OUT.json]

Renders one node of a design file with no window: by default the first layer
of the first page (a design file's frame), to FILE.png beside it. The format
follows OUT's extension unless --format is given. --layers also writes the
layer map: every visible layer's box in the image's pixels, back to front.
TESSEL_GPU=software renders on the CPU (Mesa's lavapipe).";

pub const MCP_USAGE: &str = "\
usage: tessel mcp --headless --root DIR [--images DIR]... [--open FILE]

Serves the agent tools over stdio with no window, on the design files in DIR:
list_documents, open_document, new_document and import_image, then the usual
tools on the open document. Every edit is saved to its file at once. Nothing
outside DIR is read or written, except images from each --images DIR.";

/// `tessel render …`
pub fn render_main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    match run_render(args) {
        Ok(None) => {
            println!("{RENDER_USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Some(out)) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(Usage(e)) => {
            eprintln!("tessel render: {e}\n\n{RENDER_USAGE}");
            ExitCode::from(2)
        }
        Err(Failed(e)) => {
            eprintln!("tessel render: {e}");
            ExitCode::FAILURE
        }
    }
}

enum CliError {
    Usage(String),
    Failed(String),
}
use CliError::{Failed, Usage};

fn value(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<String, CliError> {
    args.next()
        .and_then(|v| v.into_string().ok())
        .ok_or_else(|| Usage(format!("{flag} needs a value")))
}

fn run_render(args: impl IntoIterator<Item = OsString>) -> Result<Option<String>, CliError> {
    let mut args = args.into_iter();
    let mut file: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut layers: Option<PathBuf> = None;
    let mut format: Option<Format> = None;
    let mut opts = RenderOptions::default();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(None),
            Some("-o" | "--out") => out = Some(value(&mut args, "-o")?.into()),
            Some("--node") => opts.node = Some(value(&mut args, "--node")?),
            Some("--layers") => layers = Some(value(&mut args, "--layers")?.into()),
            Some("--format") => {
                let v = value(&mut args, "--format")?;
                format = Some(
                    Format::from_extension(&v)
                        .ok_or_else(|| Usage(format!("unknown format {v:?}")))?,
                );
            }
            Some("--scale") => {
                opts.scale = value(&mut args, "--scale")?
                    .parse()
                    .map_err(|_| Usage("--scale takes a number".into()))?;
            }
            Some("--background") => opts.background = Some(value(&mut args, "--background")?),
            Some("--quality") => {
                opts.quality = value(&mut args, "--quality")?
                    .parse()
                    .ok()
                    .filter(|q| (1..=100).contains(q))
                    .ok_or_else(|| Usage("--quality takes 1 to 100".into()))?;
            }
            Some(s) if s.starts_with('-') => return Err(Usage(format!("unknown option {s}"))),
            _ if file.is_none() => file = Some(arg.into()),
            _ => return Err(Usage(format!("unexpected argument {arg:?}"))),
        }
    }
    let file = file.ok_or_else(|| Usage("which file?".into()))?;
    opts.format = format
        .or_else(|| {
            out.as_ref()
                .and_then(|o| o.extension())
                .and_then(|e| Format::from_extension(&e.to_string_lossy()))
        })
        .unwrap_or(Format::Png);
    let out = out.unwrap_or_else(|| {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let stem = name
            .strip_suffix(".tessel.json")
            .unwrap_or(&name)
            .to_string();
        file.with_file_name(format!("{stem}.{}", opts.format.extension()))
    });
    let (doc, assets) = load(&file).map_err(|e| Failed(format!("{}: {e}", file.display())))?;
    let mut gpu = match opts.format {
        Format::Svg => None,
        _ => Some(Gpu::open().map_err(|e| Failed(e.to_string()))?),
    };
    let rendered = render(&doc, &assets, &opts, gpu.as_mut()).map_err(|e| Failed(e.to_string()))?;
    std::fs::write(&out, &rendered.bytes).map_err(|e| Failed(format!("{}: {e}", out.display())))?;
    if let Some(path) = &layers {
        let json = serde_json::to_string_pretty(&rendered.layers).expect("layer maps serialize");
        std::fs::write(path, json + "\n")
            .map_err(|e| Failed(format!("{}: {e}", path.display())))?;
    }
    let on = gpu
        .map(|g| format!(" on {}", g.adapter))
        .unwrap_or_default();
    Ok(Some(format!(
        "{} ({}x{}{on})",
        out.display(),
        rendered.layers.width,
        rendered.layers.height
    )))
}

/// `tessel mcp --headless …` (the arguments after `mcp`, `--headless` included).
pub fn mcp_main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut root: Option<PathBuf> = None;
    let mut images = Vec::new();
    let mut open: Option<String> = None;
    let mut args = args.into_iter();
    let usage = |e: String| {
        eprintln!("tessel mcp: {e}\n\n{MCP_USAGE}");
        ExitCode::from(2)
    };
    while let Some(arg) = args.next() {
        let got = match arg.to_str() {
            Some("--headless") => Ok(()),
            Some("-h" | "--help") => {
                println!("{MCP_USAGE}");
                return ExitCode::SUCCESS;
            }
            Some("--root") => value(&mut args, "--root").map(|v| root = Some(v.into())),
            Some("--images") => value(&mut args, "--images").map(|v| images.push(PathBuf::from(v))),
            Some("--open") => value(&mut args, "--open").map(|v| open = Some(v)),
            _ => Err(Usage(format!("unexpected argument {arg:?}"))),
        };
        if let Err(Usage(e) | Failed(e)) = got {
            return usage(e);
        }
    }
    let Some(root) = root else {
        return usage("--root is required".into());
    };
    let ws = match Workspace::new(&root, &images) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("tessel mcp: {}: {e}", root.display());
            return ExitCode::FAILURE;
        }
    };
    if let Some(path) = &open {
        if let Err(e) = futures_open(&ws, path) {
            eprintln!("tessel mcp: --open {path}: {}", e.to_json());
            return ExitCode::FAILURE;
        }
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tessel mcp: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        let agent = agent::Agent::new(ws);
        agent.app().set_notifier(agent.notifier());
        let io = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
        if let Err(e) = agent.serve(io).await {
            log::error!("{e}");
        }
    });
    ExitCode::SUCCESS
}

fn futures_open(ws: &Workspace, path: &str) -> Result<serde_json::Value, agent::ToolError> {
    use agent::AppHandle as _;
    pollster::block_on(ws.documents(agent::DocumentCall::Open {
        path: path.to_string(),
    }))
}
