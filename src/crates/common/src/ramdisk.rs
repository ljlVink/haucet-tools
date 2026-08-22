//! Commands for unpacking, repacking, inspecting, and patching ramdisk images.

use crate::compress::{compress_vec, decompress_vec};
use crate::formats::cpio::Cpio;
use crate::formats::harmony::HvbFrame;
use crate::formats::header::{FileFormat, check_fmt};
use std::fs;
use std::io;
use std::path::Path;

fn rebuild_image(
    frame: &mut HvbFrame,
    orig_payload: &[u8],
    cpio_bytes: &[u8],
    out_path: &Path,
) -> io::Result<()> {
    let fmt = check_fmt(orig_payload);
    if !fmt.is_compressed() && !matches!(fmt, FileFormat::RAW) {
        eprintln!("WARN: original payload format is {fmt}; falling back to gzip");
    }
    let use_fmt = if fmt.is_compressed() {
        fmt
    } else {
        FileFormat::GZIP
    };
    eprintln!("Recompressing with format: {use_fmt}");

    let new_payload = compress_vec(use_fmt, cpio_bytes)?;
    eprintln!("Compressed payload: {} bytes", new_payload.len());

    frame.rebuild(&new_payload);
    eprintln!(
        "New image_size={:#x}, cert_offset={:#x}, partition_size={:#x}",
        frame.footer.image_size, frame.footer.cert_offset, frame.footer.partition_size
    );
    frame.write(out_path)?;
    eprintln!(
        "Wrote {} ({} bytes)",
        out_path.display(),
        frame.footer.partition_size
    );
    Ok(())
}

pub fn patch(image_path: &Path, hsu_path: &Path, out_path: &Path) -> io::Result<()> {
    let mut frame = HvbFrame::load(image_path)?;
    let orig_payload = frame.extract_image_payload().to_vec();

    let fmt = check_fmt(&orig_payload);
    let cpio_bytes = if fmt.is_compressed() {
        decompress_vec(fmt, &orig_payload)?
    } else {
        orig_payload.clone()
    };

    let mut cpio = Cpio::load_from_data(&cpio_bytes)?;

    if cpio.exists(".backup/init_early") {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "image already patched (/.backup/init_early exists)",
        ));
    }
    if !cpio.exists("bin/init_early") {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "bin/init_early not found in ramdisk; unsupported layout",
        ));
    }

    eprintln!("Patching ramdisk:");
    cpio.mkdir(0o750, ".backup");
    cpio.mv("bin/init_early", ".backup/init_early")?;
    eprintln!("  mv bin/init_early -> .backup/init_early");
    cpio.add(0o750, "bin/init_early", path_str(hsu_path)?)?;
    eprintln!("  add bin/init_early <- {} (mode 0750)", hsu_path.display());

    // The HVB cert records the ORIGINAL compressed payload length and BL
    // truncates to it, so the final payload must not exceed the original.
    // Free space by dropping debug-only sanitizer runtimes from the ramdisk.
    for slim in ["lib64/libclang_rt.asan.so", "lib64/libclang_rt.tsan.so"] {
        if cpio.exists(slim) {
            cpio.rm(slim, false);
            eprintln!("  rm {slim} (space for ohsu)");
        }
    }

    let mut out_cpio = Vec::new();
    cpio.dump_to(&mut out_cpio)?;

    rebuild_image(&mut frame, &orig_payload, &out_cpio, out_path)?;

    let orig_len = frame.cert.image_original_len;
    let new_len = frame.footer.image_size;
    if new_len > orig_len {
        eprintln!(
            "ERROR: new image_size ({new_len:#x}) exceeds cert image_original_len ({orig_len:#x}); \
             device will truncate the payload and fail to boot. Slim the ramdisk further."
        );
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "payload exceeds cert limit",
        ));
    }
    eprintln!("  size check OK: {new_len:#x} <= {orig_len:#x}");
    Ok(())
}

pub fn unpack(image_path: &Path, workspace: &Path) -> io::Result<()> {
    let frame = HvbFrame::load(image_path)?;

    print_frame_summary(&frame);

    let payload = frame.extract_image_payload();
    if payload.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty payload"));
    }
    let payload_path = workspace.join("ramdisk.bin");
    fs::write(&payload_path, payload)?;
    eprintln!(
        "Wrote {} ({} bytes, before decompression)",
        payload_path.display(),
        payload.len()
    );

    let fmt = check_fmt(payload);
    eprintln!("Detected payload format: {fmt}");
    let cpio_bytes = if fmt.is_compressed() {
        decompress_vec(fmt, payload)?
    } else if matches!(fmt, FileFormat::RAW) {
        payload.to_vec()
    } else {
        eprintln!("WARN: payload recognised as {fmt}; treating as raw cpio");
        payload.to_vec()
    };
    let cpio_path = workspace.join("ramdisk.cpio");
    fs::write(&cpio_path, &cpio_bytes)?;
    eprintln!(
        "Wrote {} ({} bytes, decompressed)",
        cpio_path.display(),
        cpio_bytes.len()
    );

    let hdr_json = frame_to_json(&frame);
    let header_path = workspace.join("header.json");
    fs::write(&header_path, hdr_json)?;
    eprintln!("Wrote {}", header_path.display());
    Ok(())
}

pub fn repack(workspace: &Path, orig_path: &Path, out_path: &Path) -> io::Result<()> {
    let mut frame = HvbFrame::load(orig_path)?;
    let orig_payload = frame.extract_image_payload().to_vec();

    let cpio_path = workspace.join("ramdisk.cpio");
    let cpio_bytes = fs::read(&cpio_path)?;
    eprintln!("Read {} ({} bytes)", cpio_path.display(), cpio_bytes.len());

    rebuild_image(&mut frame, &orig_payload, &cpio_bytes, out_path)
}

pub fn info(image: &Path) -> io::Result<()> {
    let frame = HvbFrame::load(image)?;
    print_frame_summary(&frame);
    Ok(())
}

fn path_str(path: &Path) -> io::Result<&str> {
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not valid UTF-8: {}", path.display()),
        )
    })
}

fn print_frame_summary(frame: &HvbFrame) {
    eprintln!("--- HARMONY! header ---");
    eprintln!("  hdr_size     = 0x{:X}", frame.harmony.hdr_size);
    eprintln!("  image_size   = 0x{:X}", frame.harmony.image_size);
    eprintln!("  flags        = 0x{:X}", frame.harmony.flags);
    eprintln!("  buildvariant = {:?}", frame.harmony.buildvariant);
    eprintln!("--- HVB footer ---");
    eprintln!("  cert_offset    = 0x{:X}", frame.footer.cert_offset);
    eprintln!("  cert_size      = {}", frame.footer.cert_size);
    eprintln!("  image_size     = 0x{:X}", frame.footer.image_size);
    eprintln!("  partition_size = 0x{:X}", frame.footer.partition_size);
    eprintln!("--- HVB cert ---");
    eprintln!(
        "  version           = {}.{}",
        frame.cert.version_major, frame.cert.version_minor
    );
    eprintln!("  partition_name    = {:?}", frame.cert.partition_name);
    eprintln!(
        "  image_original_len= 0x{:X}",
        frame.cert.image_original_len
    );
    eprintln!("  image_len         = 0x{:X}", frame.cert.image_len);
    eprintln!(
        "  verity_type       = {} ({})",
        frame.cert.verity_type,
        match frame.cert.verity_type {
            1 => "hash",
            2 => "hashtree",
            _ => "?",
        }
    );
    eprintln!(
        "  hash_algo         = {} ({})",
        frame.cert.hash_algo,
        match frame.cert.hash_algo {
            0 => "SHA256",
            1 => "SHA128",
            2 => "SHA512",
            3 => "SM3",
            _ => "?",
        }
    );
    eprintln!("  salt_size         = {}", frame.cert.salt_size);
    eprintln!("  digest_size       = {}", frame.cert.digest_size);
}

fn frame_to_json(frame: &HvbFrame) -> String {
    format!(
        "{{\n\
         \"harmony\": {{ \"hdr_size\": {h_hdr}, \"image_size\": {h_img}, \"flags\": {h_flg}, \"buildvariant\": {bv:?} }},\n\
         \"footer\": {{ \"cert_offset\": {f_co}, \"cert_size\": {f_cs}, \"image_size\": {f_is}, \"partition_size\": {f_ps} }},\n\
         \"cert\": {{ \"version_major\": {c_vm}, \"version_minor\": {c_vn}, \"image_original_len\": {c_io}, \"image_len\": {c_il}, \"partition_name\": {c_pn:?}, \"verity_type\": {c_vt}, \"hash_algo\": {c_ha} }}\n\
         }}\n",
        h_hdr = frame.harmony.hdr_size,
        h_img = frame.harmony.image_size,
        h_flg = frame.harmony.flags,
        bv = frame.harmony.buildvariant,
        f_co = frame.footer.cert_offset,
        f_cs = frame.footer.cert_size,
        f_is = frame.footer.image_size,
        f_ps = frame.footer.partition_size,
        c_vm = frame.cert.version_major,
        c_vn = frame.cert.version_minor,
        c_io = frame.cert.image_original_len,
        c_il = frame.cert.image_len,
        c_pn = frame.cert.partition_name,
        c_vt = frame.cert.verity_type,
        c_ha = frame.cert.hash_algo,
    )
}
