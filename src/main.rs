//! `squeeze` CLI entry point.
//!
//! Reads command output from stdin, applies parser dispatch, writes
//! compressed output to stdout. Exit code mirrors the original command's
//! exit code (passed via --exit-code, default 0).

use anyhow::Result;
use clap::Parser;
use squeeze::core::{CompressOptions, Verbosity};
use squeeze::{compress, parsers};
use std::io::{self, Read, Write};

#[derive(Parser, Debug)]
#[command(
    name = "squeeze",
    version,
    about = "Lossless CLI output compressor for agent pipelines",
    long_about = "Reads raw command output from stdin, applies command-family-\n\
                  specific lossless compression, and writes the result to stdout.\n\
                  Omissions (if any) are always declared explicitly."
)]
struct Cli {
    /// Include stack traces in test-runner output.
    #[arg(long = "stack-traces")]
    stack_traces: bool,

    /// Include full commit bodies in git log output.
    #[arg(long = "full")]
    full: bool,

    /// Verbosity for passing tests: full, collapsed, or hidden.
    #[arg(long = "passing", default_value = "collapsed")]
    passing: String,

    /// Always show full diff content (default: true; here for explicitness).
    #[arg(long = "diff-full", default_value_t = true)]
    diff_full: bool,

    /// Hard cap on grep results (default: none).
    #[arg(long = "max-grep-results")]
    max_grep_results: Option<usize>,

    /// Show which parser would handle the input without running it.
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Append a `strategy: X` footer to output.
    #[arg(long = "strategy")]
    show_strategy: bool,

    /// Exit with this code after writing output. Mirrors the original
    /// command's exit code so tool pipelines see the right status.
    #[arg(long = "exit-code", default_value_t = 0)]
    exit_code: i32,

    /// Original command and args, after `--`. Example:
    ///   squeeze --stack-traces -- cargo test --lib
    #[arg(last = true)]
    command: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let verbosity: Verbosity = cli
        .passing
        .parse()
        .unwrap_or_else(|e: String| {
            eprintln!("squeeze: {e}; defaulting to collapsed");
            Verbosity::Collapsed
        });

    let opts = CompressOptions {
        stack_traces: cli.stack_traces,
        passing_tests: verbosity,
        diff_full: cli.diff_full,
        max_grep_results: cli.max_grep_results,
        log_full: cli.full,
    };

    let command = cli.command.join(" ");

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    if cli.dry_run {
        let strategy = detect_strategy(&command, &input);
        let stdout = io::stdout();
        let mut out = stdout.lock();
        writeln!(out, "strategy: {strategy}")?;
        writeln!(out, "input-bytes: {}", input.len())?;
        out.flush()?;
        std::process::exit(cli.exit_code);
    }

    let compressed = compress(&command, &input, &opts);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(compressed.rendered().as_bytes())?;
    if cli.show_strategy {
        writeln!(out, "\nstrategy: {}", compressed.strategy)?;
    }
    out.flush()?;

    std::process::exit(cli.exit_code);
}

/// Walk the parser registry without running parse() - used for --dry-run.
fn detect_strategy(command: &str, output: &str) -> String {
    for parser in parsers::all() {
        if parser.can_handle(command, output) {
            return parser.strategy().to_string();
        }
    }
    "passthrough".to_string()
}
