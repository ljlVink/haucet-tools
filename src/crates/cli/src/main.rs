use anyhow::{Context, Result, ensure};
use clap::{Args, CommandFactory, Parser, Subcommand};
use common::formats::cpio::{Cpio, parse_cpio_mode};
use common::formats::update_bin::{self, UpdateLayout};
use common::tools::ToolPaths;
use common::{
    formats::{erofs, rvt},
    package, ramdisk, workspace,
};
use std::fs;
use std::path::{Path, PathBuf};

const ANSI_RED: &str = "\x1b[31m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RESET: &str = "\x1b[0m";

#[derive(Debug, Parser)]
#[command(version, about, arg_required_else_help = true)]
pub struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    tools_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Unpack an update package into a workspace directory
    #[command(arg_required_else_help = true)]
    Unpack(FullUnpackArgs),
    /// List or unpack an update.bin file
    #[command(arg_required_else_help = true)]
    UpdateBin {
        #[command(subcommand)]
        command: UpdateBinCommand,
    },
    /// Unpack or repack an EROFS image
    #[command(arg_required_else_help = true)]
    Erofs {
        #[command(subcommand)]
        command: ErofsCommand,
    },
    /// Run commands on a cpio archive and save changes in place
    #[command(arg_required_else_help = true)]
    Cpio {
        /// Input cpio archive, modified in place
        incpio: PathBuf,
        #[command(subcommand)]
        command: CpioCommands,
    },
    /// Parse and inspect an RVT image
    #[command(arg_required_else_help = true)]
    Rvt { file: PathBuf },
    /// Unpack, repack, patch, or inspect a ramdisk image
    #[command(arg_required_else_help = true)]
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
    /// List the contents of an update binary
    List {
        input: PathBuf,
        #[arg(long, value_enum, default_value_t = UpdateLayout::Auto)]
        layout: UpdateLayout,
    },
    /// Unpack an update binary into a workspace directory
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
    /// Unpack an EROFS image into a workspace directory
    Unpack {
        image: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, help = "Keep extracted ownership and modes unchanged")]
        skip_chown: bool,
    },
    /// Repack an EROFS workspace into an image
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
    /// Print the HARMONY! header and HVB footer/certificate fields
    Info { image: PathBuf },
}

#[derive(Debug, Subcommand)]
enum CpioCommands {
    /// Exit 0 if an entry exists, otherwise 1
    Exists {
        /// Archive entry to check
        entry: String,
    },
    /// List entries in the archive
    Ls {
        /// List entries recursively
        #[arg(short = 'r', long)]
        recursive: bool,
        /// Entry path; defaults to `/`
        path: Option<String>,
    },
    /// Remove an entry from the archive
    Rm {
        /// Remove recursively
        #[arg(short = 'r', long)]
        recursive: bool,
        /// Archive entry to remove
        entry: String,
    },
    /// Create a directory in the archive
    Mkdir {
        /// Directory mode in octal, such as `0750`
        mode: String,
        /// Directory entry to create
        entry: String,
    },
    /// Create a symbolic link in the archive
    Ln {
        /// Link target
        src: String,
        /// Link entry to create
        dst: String,
    },
    /// Move an entry in the archive
    Mv {
        /// Source entry
        src: String,
        /// Destination entry
        dst: String,
    },
    /// Add a file to the archive
    Add {
        /// File mode in octal, such as `0750`
        mode: String,
        /// Archive entry to create
        entry: String,
        /// Input file on the host filesystem
        infile: String,
    },
    /// Extract all entries or one entry from the archive
    Extract {
        /// Archive entry to extract; omit together with OUT to extract everything
        #[arg(requires = "out")]
        entry: Option<String>,
        /// Host path to write the extracted entry to
        #[arg(requires = "entry")]
        out: Option<String>,
    },
    /// Exit 0 for stock, 1 for patched, or 2 for unsupported
    Test,
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
        Command::Cpio { incpio, command } => run_cpio_command(&incpio, command),
        Command::Rvt { file } => {
            let file = file
                .to_str()
                .with_context(|| format!("RVT path is not valid UTF-8: {}", file.display()))?;
            Ok(rvt::parse_file(file)?)
        }
        Command::Ramdisk { command } => run_ramdisk_command(command),
    }
}

fn run_cpio_command(file: &Path, command: CpioCommands) -> Result<()> {
    let file_str = file
        .to_str()
        .with_context(|| format!("cpio path is not valid UTF-8: {}", file.display()))?;
    let mut cpio = if file.exists() {
        Cpio::load_from_file(file_str)?
    } else {
        Cpio::new()
    };

    let status = match command {
        CpioCommands::Exists { entry } => i32::from(!cpio.exists(&entry)),
        CpioCommands::Ls { recursive, path } => {
            cpio.ls(path.as_deref().unwrap_or("/"), recursive);
            0
        }
        CpioCommands::Rm { recursive, entry } => {
            cpio.rm(&entry, recursive);
            cpio.dump(file_str)?;
            0
        }
        CpioCommands::Mkdir { mode, entry } => {
            cpio.mkdir(parse_cpio_mode(&mode)?, &entry);
            cpio.dump(file_str)?;
            0
        }
        CpioCommands::Ln { src, dst } => {
            cpio.ln(&src, &dst);
            cpio.dump(file_str)?;
            0
        }
        CpioCommands::Mv { src, dst } => {
            cpio.mv(&src, &dst)?;
            cpio.dump(file_str)?;
            0
        }
        CpioCommands::Add {
            mode,
            entry,
            infile,
        } => {
            cpio.add(parse_cpio_mode(&mode)?, &entry, &infile)?;
            cpio.dump(file_str)?;
            0
        }
        CpioCommands::Extract { entry, out } => {
            match (entry.as_deref(), out.as_deref()) {
                (None, None) => {
                    cpio.extract(&[])?;
                }
                (Some(entry), Some(out)) => cpio.extract(&[entry, out])?,
                _ => unreachable!("clap requires extract entry and output together"),
            }
            0
        }
        CpioCommands::Test => {
            if cpio.exists(".backup/init_early") {
                1
            } else if cpio.exists("bin/init_early") || cpio.exists("init") {
                0
            } else {
                2
            }
        }
    };

    if status != 0 {
        std::process::exit(status);
    }
    Ok(())
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
