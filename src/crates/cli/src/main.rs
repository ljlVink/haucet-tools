use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use common::tools::ToolPaths;
use common::update_bin::{self, UpdateLayout};
use common::{erofs, package, workspace};
use std::path::PathBuf;

const ANSI_RED: &str = "\x1b[31m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RESET: &str = "\x1b[0m";

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    tools_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Unpack(FullUnpackArgs),
    UpdateBin {
        #[command(subcommand)]
        command: UpdateBinCommand,
    },
    Erofs {
        #[command(subcommand)]
        command: ErofsCommand,
    },
    #[command(trailing_var_arg = true)]
    Ramdisk {
        #[arg(required = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Debug, Args)]
struct FullUnpackArgs {
    input: PathBuf,
    #[arg(short, long)]
    out: PathBuf,
    #[arg(short = 'p', long = "partition")]
    partitions: Vec<String>,
    #[arg(long, conflicts_with = "partitions")]
    all_erofs: bool,
    #[arg(long, value_enum, default_value_t = UpdateLayout::Auto)]
    layout: UpdateLayout,
    #[arg(long)]
    force: bool,
    #[arg(long, help = "Keep extracted ownership and modes unchanged")]
    skip_chown: bool,
}

#[derive(Debug, Subcommand)]
enum UpdateBinCommand {
    List {
        input: PathBuf,
        #[arg(long, value_enum, default_value_t = UpdateLayout::Auto)]
        layout: UpdateLayout,
    },
    Unpack {
        input: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t = UpdateLayout::Auto)]
        layout: UpdateLayout,
        #[arg(long)]
        force: bool,
        #[arg(long, help = "Keep extracted ownership and modes unchanged")]
        skip_chown: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ErofsCommand {
    Unpack {
        image: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, help = "Keep extracted ownership and modes unchanged")]
        skip_chown: bool,
    },
    Repack {
        workspace: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        allow_grow: bool,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    let tools_dir = cli.tools_dir;
    match cli.command {
        Command::Unpack(args) => {
            let tools = ToolPaths::discover(tools_dir)?;
            package::unpack_full(
                &args.input,
                &args.out,
                &tools,
                &args.partitions,
                args.all_erofs,
                args.layout,
                args.force,
            )?;
            finish_unpack(&args.out, args.skip_chown)
        }
        Command::UpdateBin { command } => match command {
            UpdateBinCommand::List { input, layout } => update_bin::list_file(&input, layout),
            UpdateBinCommand::Unpack {
                input,
                out,
                layout,
                force,
                skip_chown,
            } => {
                update_bin::unpack_file(&input, &out, layout, force)?;
                finish_unpack(&out, skip_chown)
            }
        },
        Command::Erofs { command } => {
            let tools = ToolPaths::discover(tools_dir)?;
            match command {
                ErofsCommand::Unpack {
                    image,
                    out,
                    force,
                    skip_chown,
                } => {
                    erofs::unpack(&image, &out, &tools, force)?;
                    finish_unpack(&out, skip_chown)
                }
                ErofsCommand::Repack {
                    workspace,
                    output,
                    allow_grow,
                } => erofs::repack(&workspace, &output, &tools, allow_grow),
            }
        }
        Command::Ramdisk { args } => {
            let mut forwarded = vec!["haucet-tools ramdisk".to_owned()];
            forwarded.extend(args);
            let code = common::ramdisk::run(&forwarded);
            if code == 0 {
                Ok(())
            } else {
                bail!("ramdisk command exited with status {code}")
            }
        }
    }
}

fn finish_unpack(out: &std::path::Path, skip_chown: bool) -> Result<()> {
    if skip_chown {
        Ok(())
    } else {
        workspace::make_invoking_user_writable(out)
    }
}

fn main() {
    if unsafe { libc::geteuid() == 0 } {
        eprintln!("{ANSI_RED}You are currently in root mode, use it at risk.{ANSI_RESET}");
    } else {
        eprintln!(
            "{ANSI_YELLOW}You are currently not in root mode, extract may cause permission problems.{ANSI_RESET}"
        );
    }
    let main_cli = Cli::parse();
    if let Err(error) = run(main_cli) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
