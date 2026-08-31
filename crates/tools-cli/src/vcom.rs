use anyhow::{Context, Result, ensure};
use hisi_vcom::transport::{self, DeviceFilter, SerialVcomDevice};
use hisi_vcom::vcom;
use std::cell::Cell;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

pub fn devices() -> Result<()> {
    let mut found = false;

    for port in transport::list_serial_ports().context("failed to enumerate serial ports")? {
        found = true;
        println!("{:<8}  {}", port.name, port.description);
    }

    let filter = DeviceFilter {
        vid: Some(0x12D1),
        ..Default::default()
    };
    for line in transport::list_candidates(&filter).context("failed to enumerate USB devices")? {
        found = true;
        println!("USB      {line}");
    }

    if !found {
        println!("No VCOM devices found.");
    }
    Ok(())
}

pub fn flash(port: &str, address: u32, file: &Path) -> Result<()> {
    let data = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    validate_loader(&data, file)?;
    let mut device = SerialVcomDevice::open(port, 115200)
        .with_context(|| format!("opening VCOM port {port}"))?;
    let interactive = io::stdout().is_terminal();
    let progress_active = Cell::new(false);
    let mut log = |message: &str| {
        if progress_active.replace(false) {
            println!();
        }
        println!("* {message}");
    };
    let mut last_progress_bucket = 0;

    let result = vcom::upload(&mut device, &data, address, &mut log, &mut |sent, total| {
        if interactive {
            print_progress(sent, total);
            progress_active.set(true);
        } else if should_report_progress(sent, total, &mut last_progress_bucket) {
            println!("  {sent}/{total} bytes");
        }
    });
    if progress_active.replace(false) {
        println!();
    }
    result?;
    println!("Flash finished.");
    Ok(())
}

fn print_progress(sent: u64, total: u64) {
    const BAR_WIDTH: usize = 30;

    let sent = sent.min(total);
    let percent = if total == 0 { 100 } else { sent * 100 / total };
    let filled = if total == 0 {
        BAR_WIDTH
    } else {
        (sent * BAR_WIDTH as u64 / total) as usize
    };
    let bar = format!("{}{}", "#".repeat(filled), "-".repeat(BAR_WIDTH - filled));

    print!("\r  [{bar}] {percent:3}%  {sent}/{total} bytes");
    let _ = io::stdout().flush();
}

fn validate_loader(data: &[u8], file: &Path) -> Result<()> {
    ensure!(!data.is_empty(), "loader file is empty: {}", file.display());
    Ok(())
}

fn should_report_progress(sent: u64, total: u64, last_bucket: &mut u64) -> bool {
    if total == 0 {
        return false;
    }

    let bucket = sent.min(total).saturating_mul(10) / total;
    if bucket <= *last_bucket {
        return false;
    }
    *last_bucket = bucket;
    true
}
