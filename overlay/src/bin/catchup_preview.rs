//! Prints what the overlay would show, without the game.
//!
//!     catchup_preview <repo> [player] [max_lines]

use std::path::Path;
use std::process::ExitCode;

use catchup_overlay::{catchup, view};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(repo) = args.first() else {
        eprintln!("usage: catchup_preview <repo> [player] [max_lines]");
        return ExitCode::FAILURE;
    };
    let max_lines = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(25);
    match catchup::load(Path::new(repo), args.get(1).map(String::as_str).filter(|p| !p.is_empty())) {
        Ok(c) => {
            for l in view::lines(&c, max_lines) {
                let indent = if l.style == view::Style::Item { "  " } else { "" };
                let aside = if l.aside.is_empty() { String::new() } else { format!("  ({})", l.aside) };
                println!("{indent}{}{aside}", l.text);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
