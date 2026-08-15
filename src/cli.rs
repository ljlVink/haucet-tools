use crate::erofs;
use crate::package;
use crate::tools::ToolPaths;
use crate::update_bin::{self, UpdateLayout};
use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Directory containing extract.erofs and mkfs.erofs.
    #[arg(long, global = true, value_name = "DIR")]
    tools_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Unpack update_full_base.zip and auto-detect EROFS/ramdisk partitions.
    Unpack(FullUnpackArgs),
    /// Inspect or unpack a Huawei update.bin file.
    UpdateBin {
        #[command(subcommand)]
        command: UpdateBinCommand,
    },
    /// Unpack or repack an EROFS partition image.
    Erofs {
        #[command(subcommand)]
        command: ErofsCommand,
    },
    /// Run a ramdisk-tools action (unpack, repack, info, cpio, rvt, ramdiskpatch).
    #[command(trailing_var_arg = true)]
    Ramdisk {
        #[arg(required = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Debug, Args)]
struct FullUnpackArgs {
    /// Huawei update_full_base.zip input.
    input: PathBuf,
    /// Workspace output directory.
    #[arg(short, long)]
    out: PathBuf,
    /// Partition to unpack. Repeat to override automatic format detection.
    #[arg(short = 'p', long = "partition")]
    partitions: Vec<String>,
    /// Unpack every component detected as EROFS.
    #[arg(long, conflicts_with = "partitions")]
    all_erofs: bool,
    /// Huawei component table layout.
    #[arg(long, value_enum, default_value_t = UpdateLayout::Auto)]
    layout: UpdateLayout,
    /// Replace existing extracted files.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Subcommand)]
enum UpdateBinCommand {
    /// List update.bin components without extracting them.
    List {
        input: PathBuf,
        #[arg(long, value_enum, default_value_t = UpdateLayout::Auto)]
        layout: UpdateLayout,
    },
    /// Extract update.bin components using bounded streaming I/O.
    Unpack {
        input: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t = UpdateLayout::Auto)]
        layout: UpdateLayout,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ErofsCommand {
    /// Extract the filesystem tree and repack metadata.
    Unpack {
        image: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Rebuild an EROFS image from an unpack workspace.
    Repack {
        workspace: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Permit an image without an HVB wrapper to grow beyond its original size.
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
            )
        }
        Command::UpdateBin { command } => match command {
            UpdateBinCommand::List { input, layout } => update_bin::list_file(&input, layout),
            UpdateBinCommand::Unpack {
                input,
                out,
                layout,
                force,
            } => update_bin::unpack_file(&input, &out, layout, force).map(|_| ()),
        },
        Command::Erofs { command } => {
            let tools = ToolPaths::discover(tools_dir)?;
            match command {
                ErofsCommand::Unpack { image, out, force } => {
                    erofs::unpack(&image, &out, &tools, force)
                }
                ErofsCommand::Repack {
                    workspace,
                    output,
                    allow_grow,
                } => erofs::repack(&workspace, &output, &tools, allow_grow),
            }
        }
        Command::Ramdisk { args } => {
            let mut forwarded = vec!["ramdisk-tools".to_owned()];
            forwarded.extend(args);
            let code = ramdisk_tools::cli::run(&forwarded);
            if code == 0 {
                Ok(())
            } else {
                bail!("ramdisk-tools exited with status {code}")
            }
        }
    }
}
