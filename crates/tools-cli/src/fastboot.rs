use anyhow::{Context, Result, bail};
use hm_fastboot::nusb::{FlashEvent, NusbFastBoot, clean_device_string, require_single_device};
use std::path::Path;

pub async fn devices() -> Result<()> {
    let devices = hm_fastboot::nusb::devices()
        .await
        .context("failed to enumerate USB devices")?;
    let mut found = false;
    for info in devices {
        found = true;
        let product = display_device_string(info.product_string());
        let serial = display_device_string(info.serial_number());
        println!(
            "{:>3}:{:<3}  {:#06x}:{:#06x}  {}  serial={}",
            info.bus_id(),
            info.device_address(),
            info.vendor_id(),
            info.product_id(),
            product,
            serial,
        );
    }
    if !found {
        bail!("cannot find fastboot devices");
    }
    Ok(())
}

pub async fn get_var(var: &str) -> Result<()> {
    let mut fb = open_only().await?;
    let value = fb
        .get_var(var)
        .await
        .with_context(|| format!("read {var} fail"))?;
    println!("{var}: {value}");
    Ok(())
}

pub async fn flash(partition: &str, image: &Path) -> Result<()> {
    let mut fb = open_only().await?;
    let mut progress = |event: FlashEvent<'_>| match event {
        FlashEvent::Message(msg) => println!("{msg}"),
        FlashEvent::Part { index, total } => println!("Progress: {index}/{total} parts completed"),
    };
    fb.flash_image(partition, image, &mut progress)
        .await
        .with_context(|| format!("failed to flash {} to {}", image.display(), partition))?;
    println!("Flash completed");
    Ok(())
}

pub async fn reboot() -> Result<()> {
    let mut fb = open_only().await?;
    fb.reboot().await.context("failed to send reboot command")?;
    println!("Reboot command sent");
    Ok(())
}

pub async fn oem(args: &[String]) -> Result<()> {
    let mut fb = open_only().await?;
    let command = args.join(" ");
    let lines = fb
        .oem(&command)
        .await
        .with_context(|| format!("failed to send OEM command: {command}"))?;
    for line in lines {
        println!("{line}");
    }
    println!("OEM command completed");
    Ok(())
}

async fn open_only() -> Result<NusbFastBoot> {
    let devices = hm_fastboot::nusb::devices()
        .await
        .context("failed to enumerate USB devices")?;
    let info = require_single_device(devices)?;
    let product = display_device_string(info.product_string());
    eprintln!(
        "Using device {}:{} ({})",
        info.bus_id(),
        info.device_address(),
        product,
    );
    NusbFastBoot::from_info(&info)
        .await
        .context("failed to open fastboot device (administrator privileges or a WinUSB driver may be required)")
}

fn display_device_string(value: Option<&str>) -> String {
    value
        .map(|value| clean_device_string(value).unwrap_or_else(|| value.to_owned()))
        .unwrap_or_default()
}
