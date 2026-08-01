use clap::CommandFactory;
use clap_complete::aot::{generate, Shell};

use crate::cli::Cli;

/// Print a completion script for `shell` to stdout.
///
/// Deliberately static (clap's ahead-of-time generator) rather than dynamic:
/// a dynamic completer would have to start a process and reach the desktop app
/// on every Tab, which means a stalled prompt whenever the app is closed or
/// busy. Subcommands, flags and fixed value sets cover nearly all of the
/// keystroke saving anyway; document IDs — the one thing worth completing
/// dynamically — are opaque and unguessable regardless.
pub fn run(shell: Shell) -> anyhow::Result<()> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
    Ok(())
}
