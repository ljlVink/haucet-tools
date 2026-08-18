mod cli;
mod erofs;
mod hvb;
mod package;
mod tools;
mod update_bin;
mod workspace;

use clap::Parser;

const ANSI_RED: &str = "\x1b[31m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RESET: &str = "\x1b[0m";

fn main() {
    show_privilege_status();
    let cli = cli::Cli::parse();
    if let Err(error) = cli::run(cli) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn show_privilege_status() {
    if is_superuser() {
        eprintln!("{ANSI_RED}You are currently in root mode, use it at risk.{ANSI_RESET}");
    } else {
        eprintln!(
            "{ANSI_YELLOW}You are currently not in root mode, extract may cause permission problems.{ANSI_RESET}"
        );
    }
}

fn is_superuser() -> bool {
    {
        unsafe { libc::geteuid() == 0 }
    }
}
