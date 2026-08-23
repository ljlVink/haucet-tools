use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use hm_fastboot::nusb::{clean_device_string, FlashEvent, NusbFastBoot};

#[derive(Parser)]
enum Opts {
    GetVar { var: String },
    GetAllVars {},
    Flash { target: String, file: PathBuf },
    Reboot,
    Oem { command: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let opts = Opts::parse();

    let mut devices = hm_fastboot::nusb::devices().await?;
    let info = devices
        .next()
        .ok_or_else(|| anyhow::anyhow!("No Device found"))?;

    let product = info
        .product_string()
        .map(|s| clean_device_string(s).unwrap_or_else(|| s.to_string()));

    println!(
        "Using Fastboot device: {}:{} P: {}",
        info.bus_id(),
        info.device_address(),
        product.as_deref().unwrap_or_default()
    );

    let mut fb = NusbFastBoot::from_info(&info).await?;

    match opts {
        Opts::GetVar { var } => {
            let r = fb.get_var(&var).await?;
            println!("{var}: {r:?}");
        }
        Opts::GetAllVars {} => {
            let r = fb.get_all_vars().await?;
            for (k, v) in r {
                println!("{k}: {v}");
            }
        }
        Opts::Flash { target, file } => {
            let mut progress = |event: FlashEvent<'_>| match event {
                FlashEvent::Message(msg) => println!("{msg}"),
                FlashEvent::Part { index, total } => {
                    println!("Part {index}/{total} done");
                }
            };
            fb.flash_image(&target, &file, &mut progress)
                .await
                .with_context(|| format!("Flashing {} to {}", file.display(), target))?;
            println!("Flashing done");
        }
        Opts::Reboot => fb.reboot().await?,
        Opts::Oem { command } => {
            let lines = fb.oem(&command).await?;
            for line in lines {
                println!("{line}");
            }
        }
    }

    Ok(())
}
