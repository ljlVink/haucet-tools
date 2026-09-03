use anyhow::{Context, Result, bail};
use hm_fastboot::nusb::{
    FlashEvent, NusbFastBoot, NusbFastBootError, clean_device_string, require_single_device,
};
use std::fs::File;
use std::io::{Read, Write};
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

    match fb.ultraflash(partition).await {
        Ok(()) => {
            println!("Using Ultraflash protocol!");
            let download_result = download_image(&mut fb, image).await;
            let stop_result = fb.ultraflash_stop().await;

            if let Err(error) = download_result {
                return match stop_result {
                    Ok(()) => Err(error),
                    Err(stop_error) => Err(anyhow::anyhow!(
                        "failed to download {}: {error}; additionally failed to stop ultraflash: {stop_error}",
                        image.display()
                    )),
                };
            }
            stop_result.context("failed to stop ultraflash mode")?;
            println!("Flash completed");
            return Ok(());
        }
        Err(NusbFastBootError::FastbootFailed(_)) => {
            println!("Ultraflash is not supported; using standard fastboot flash");
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to probe ultraflash support for partition {partition}")
            });
        }
    }

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

async fn download_image(fb: &mut NusbFastBoot, image: &Path) -> Result<()> {
    let mut file = File::open(image)
        .with_context(|| format!("failed to open download image {}", image.display()))?;
    let size = u32::try_from(
        file.metadata()
            .with_context(|| format!("failed to stat download image {}", image.display()))?
            .len(),
    )
    .with_context(|| format!("download image is larger than 4 GiB: {}", image.display()))?;

    let mut sender = fb
        .download(size)
        .await
        .with_context(|| format!("failed to start download of {}", image.display()))?;
    while sender.left() > 0 {
        let amount = sender.left().min(1024 * 1024) as usize;
        let buffer = sender
            .get_mut_data(amount)
            .await
            .context("failed to allocate fastboot download buffer")?;
        file.read_exact(buffer)
            .with_context(|| format!("failed to read download image {}", image.display()))?;
    }
    sender
        .finish()
        .await
        .with_context(|| format!("failed to finish download of {}", image.display()))?;
    println!("Downloaded {} ({} bytes)", image.display(), size);
    Ok(())
}

pub async fn upload_memory(params: &str, output: &Path) -> Result<()> {
    let (address, length) = params
        .split_once(':')
        .context("upload-memory parameters must be ADDRESS:LENGTH")?;
    if address.is_empty() || length.is_empty() || length.contains(':') {
        bail!("upload-memory parameters must be ADDRESS:LENGTH");
    }
    parse_hex_u64(address).with_context(|| format!("invalid upload-memory address: {address}"))?;
    let size =
        parse_hex_u32(length).with_context(|| format!("invalid upload-memory length: {length}"))?;

    let mut fb = open_only().await?;
    let data = fb
        .upload_memory(params, size)
        .await
        .with_context(|| format!("failed to upload memory range {params}"))?;
    let mut file = File::create(output)
        .with_context(|| format!("failed to create output file {}", output.display()))?;
    file.write_all(&data)
        .with_context(|| format!("failed to write output file {}", output.display()))?;
    println!(
        "Uploaded memory range {params} to {} ({} bytes)",
        output.display(),
        data.len()
    );
    Ok(())
}

fn parse_hex_u32(value: &str) -> Result<u32, std::num::ParseIntError> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u32::from_str_radix(value, 16)
}

fn parse_hex_u64(value: &str) -> Result<u64, std::num::ParseIntError> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u64::from_str_radix(value, 16)
}

pub async fn erase(partition: &str) -> Result<()> {
    let mut fb = open_only().await?;
    fb.erase(partition)
        .await
        .with_context(|| format!("failed to erase partition {partition}"))?;
    println!("Erased {partition}");
    Ok(())
}

pub async fn reboot_bootloader() -> Result<()> {
    let mut fb = open_only().await?;
    fb.reboot_bootloader()
        .await
        .context("failed to reboot into bootloader")?;
    println!("Reboot into bootloader command sent");
    Ok(())
}

pub async fn reboot_recovery() -> Result<()> {
    let mut fb = open_only().await?;
    fb.reboot_recovery()
        .await
        .context("failed to reboot into recovery")?;
    println!("Reboot into recovery command sent");
    Ok(())
}

pub async fn reboot_fastboot() -> Result<()> {
    let mut fb = open_only().await?;
    fb.reboot_fastboot()
        .await
        .context("failed to reboot into fastboot")?;
    println!("Reboot into fastboot command sent");
    Ok(())
}

pub async fn continue_boot() -> Result<()> {
    let mut fb = open_only().await?;
    fb.continue_boot()
        .await
        .context("failed to send continue command")?;
    println!("Continue command sent");
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
