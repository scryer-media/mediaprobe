//! Repository tooling. `cargo xtask test-corpus <command>`; see
//! docs/test-corpus.md.

mod corpus;
mod recipes;

use std::process::ExitCode;

const USAGE: &str =
    "usage: cargo xtask test-corpus <command> [options]  (cargo xtask test-corpus help)";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let result = match (args.next().as_deref(), args.next()) {
        (Some("test-corpus"), Some(command)) => {
            corpus::commands::dispatch(&command, corpus::commands::Args::new(args.collect()))
        }
        (Some("test-corpus"), None) => {
            print!("{}", corpus::commands::USAGE);
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
