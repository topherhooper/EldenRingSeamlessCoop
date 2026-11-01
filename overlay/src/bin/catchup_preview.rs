//! Prints what the overlay would show, without the game: the hint line, then the menu's list.
//!
//!     catchup_preview <repo> [player]

use std::path::Path;
use std::process::ExitCode;

use catchup_overlay::{catchup, local_now, view};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(repo) = args.first() else {
        eprintln!("usage: catchup_preview <repo> [player]");
        return ExitCode::FAILURE;
    };
    match catchup::load(
        Path::new(repo),
        args.get(1).map(String::as_str).filter(|p| !p.is_empty()),
    ) {
        Ok(c) => {
            let (text, aside) = view::compact(&c);
            println!("Hint: Catch-up  {text}  {aside}\n");
            for l in view::lines(&c, local_now()) {
                let indent = if l.style == view::Style::Item {
                    "  "
                } else {
                    ""
                };
                let aside = if l.aside.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", l.aside)
                };
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
