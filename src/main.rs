use std::io::Write;

use clap::Parser;

fn main() {
    let cli = covenant::cli::Cli::parse();
    let code = covenant::cli::run(cli);
    // process::exit skips destructors; CI pipes are block-buffered, so an
    // unflushed report would vanish exactly where the exit code matters.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}
