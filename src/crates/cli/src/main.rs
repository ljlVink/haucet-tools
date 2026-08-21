use anyhow::{Context, Result, ensure};
use clap::{Args, CommandFactory, Parser, Subcommand};
use common::formats::update_bin::{self, UpdateLayout};
use common::tools::ToolPaths;
use common::{formats::erofs, package, ramdisk, workspace};
use std::fs;
use std::path::{Path, PathBuf};

const ANSI_RED: &str = "\x1b[31m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RESET: &str = "\x1b[0m";
const CPIO_LONG_ABOUT: &str = "Run commands on a cpio archive and save modifications in place.

Each command must be passed as one quoted argument.

Commands:
  exists ENTRY            exit 0 if the entry exists, otherwise 1
  ls [-r] [PATH]          list entries
  rm [-r] ENTRY           remove an entry
  mkdir MODE ENTRY        create a directory
  ln SRC DST              create a symbolic link
  mv SRC DST              move an entry
  add MODE ENTRY INFILE   add a file
  extract [ENTRY OUT]     extract all entries or one entry
  test                    exit 0 for stock, 1 for patched, or 2 unsupported

Examples:
  haucet-tools ramdisk cpio ramdisk.cpio \"ls -r /\"
  haucet-tools ramdisk cpio ramdisk.cpio \"rm -r path\" \"add 0750 path file\"";

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
    Ramdisk {
        #[command(subcommand)]
        command: Option<RamdiskCommand>,
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

#[derive(Debug, Subcommand)]
enum RamdiskCommand {
    /// Unpack an HMOS ramdisk image into a workspace directory
    Unpack {
        image: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, help = "Keep extracted ownership and modes unchanged")]
        skip_chown: bool,
    },
    /// Repack a ramdisk workspace using its original image
    Repack {
        workspace: PathBuf,
        original_image: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Patch bin/init_early in a ramdisk image
    #[command(name = "ramdiskpatch")]
    Patch {
        image: PathBuf,
        binary: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Run commands on a cpio archive and save it in place
    #[command(long_about = CPIO_LONG_ABOUT)]
    Cpio {
        #[arg(help = "Input cpio archive, modified in place")]
        incpio: PathBuf,
        #[arg(
            value_name = "COMMAND",
            help = "Quoted cpio command; may be repeated",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        commands: Vec<String>,
    },
    /// Print the HARMONY! header and HVB footer/certificate fields
    Info { image: PathBuf },
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
        Command::Ramdisk { command } => run_ramdisk_command(command),
    }
}

fn run_ramdisk_command(command: Option<RamdiskCommand>) -> Result<()> {
    match command {
        None => print_ramdisk_help(),
        Some(RamdiskCommand::Unpack {
            image,
            out,
            force,
            skip_chown,
        }) => {
            let image = canonical_path(&image)?;
            prepare_output_dir(&out, force)?;
            ramdisk::unpack(&image, &out)?;
            finish_unpack(&out, skip_chown)
        }
        Some(RamdiskCommand::Repack {
            workspace,
            original_image,
            out,
        }) => {
            let workspace = canonical_path(&workspace)?;
            let original_image = canonical_path(&original_image)?;
            let out = absolute_path(&out)?;
            Ok(ramdisk::repack(&workspace, &original_image, &out)?)
        }
        Some(RamdiskCommand::Patch { image, binary, out }) => {
            Ok(ramdisk::patch(&image, &binary, &out)?)
        }
        Some(RamdiskCommand::Cpio { incpio, commands }) => {
            let status = ramdisk::edit_cpio(&incpio, &commands)?;
            if status != 0 {
                std::process::exit(status);
            }
            Ok(())
        }
        Some(RamdiskCommand::Info { image }) => Ok(ramdisk::info(&image)?),
    }
}

fn print_ramdisk_help() -> Result<()> {
    let mut command = Cli::command();
    let ramdisk = command
        .find_subcommand_mut("ramdisk")
        .context("ramdisk command is missing")?;
    ramdisk.print_help()?;
    println!();
    Ok(())
}

fn prepare_output_dir(output: &Path, force: bool) -> Result<()> {
    if output.exists() && fs::read_dir(output)?.next().is_some() {
        ensure!(force, "output directory is not empty: {}", output.display());
        fs::remove_dir_all(output)
            .with_context(|| format!("removing old output directory {}", output.display()))?;
    }
    fs::create_dir_all(output)
        .with_context(|| format!("creating output directory {}", output.display()))
}

fn canonical_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("resolving {}", path.display()))
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
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
