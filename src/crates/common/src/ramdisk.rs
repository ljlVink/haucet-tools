//! Commands for unpacking, repacking, inspecting, and patching ramdisk images.

use crate::compress::{compress_vec, decompress_vec};
use crate::formats::cpio::Cpio;
use crate::formats::harmony::HvbFrame;
use crate::formats::header::{FileFormat, check_fmt};
use crate::rvt;
use std::fs;
use std::io;
use std::path::Path;

fn rebuild_image(
    frame: &mut HvbFrame,
    orig_payload: &[u8],
    cpio_bytes: &[u8],
    out_path: &str,
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
    eprintln!("Wrote {out_path} ({} bytes)", frame.footer.partition_size);
    Ok(())
}

fn ramdiskpatch_cmd(args: &[String]) -> io::Result<()> {
    if args.len() < 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ramdiskpatch: needs <image> <binary> [out_image]",
        ));
    }
    let image_path = &args[0];
    let hsu_path = &args[1];
    let out_path = if args.len() >= 3 {
        args[2].clone()
    } else {
        "new-ramdisk.img".to_string()
    };

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
    cpio.add(0o750, "bin/init_early", hsu_path)?;
    eprintln!("  add bin/init_early <- {hsu_path} (mode 0750)");

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

    rebuild_image(&mut frame, &orig_payload, &out_cpio, &out_path)?;

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

const USAGE: &str = "haucet-tools ramdisk - HMOS boot/ramdisk image tool\n\
\n\
Usage: haucet-tools ramdisk <action> [args...]\n\
\n\
Supported actions:\n\
  unpack <image>\n\
    Unpack an HMOS ramdisk image into the current directory:\n\
      header.json   - HARMONY! header and HVB footer/certificate summary\n\
      ramdisk.cpio  - decompressed newc cpio archive\n\
      ramdisk.bin   - original compressed ramdisk payload\n\
\n\
  repack <orig_image> [out_image]\n\
    Repack the current directory's `ramdisk.cpio` using the compression format\n\
    from <orig_image>. The HVB certificate is preserved byte-for-byte.\n\
    [out_image] defaults to `new-ramdisk.img`.\n\
\n\
  ramdiskpatch <image> <binary> [out_image]\n\
    Back up `bin/init_early` to `.backup/init_early`, install <binary> as\n\
    `bin/init_early` with mode 0750, and preserve the HVB certificate.\n\
    [out_image] defaults to `new-ramdisk.img`.\n\
\n\
  cpio <incpio> [commands...]\n\
    Run commands on a cpio archive and save modifications in place.\n\
    Each command must be passed as one quoted argument.\n\
    Commands:\n\
      exists ENTRY            exit 0 if the entry exists, otherwise 1\n\
      ls [-r] [PATH]          list entries\n\
      rm [-r] ENTRY           remove an entry\n\
      mkdir MODE ENTRY        create a directory\n\
      ln SRC DST              create a symbolic link\n\
      mv SRC DST              move an entry\n\
      add MODE ENTRY INFILE   add a file\n\
      extract [ENTRY OUT]     extract all entries or one entry\n\
      test                    exit 0 for stock, 1 for patched, or 2 unsupported\n\
\n\
  info <image>\n\
    Print the HARMONY! header and HVB footer/certificate fields.\n\
\n\
  rvt <image>\n\
    Parse a raw or HVB-wrapped RVT image without modifying it.\n";

pub fn run(args: &[String]) -> i32 {
    if args.len() < 2 {
        eprint!("{USAGE}");
        return 1;
    }
    let action = args[1].as_str();
    let rest = &args[2..];
    let result = match action {
        "unpack" => unpack_cmd(rest),
        "repack" => repack_cmd(rest),
        "ramdiskpatch" => ramdiskpatch_cmd(rest),
        "cpio" => cpio_cmd(rest),
        "info" => info_cmd(rest),
        "rvt" => rvt_cmd(rest),
        "-h" | "--help" | "help" => {
            eprint!("{USAGE}");
            Ok(())
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown action: {other}"),
        )),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("haucet-tools ramdisk: {action} failed: {e}");
            1
        }
    }
}

fn rvt_cmd(args: &[String]) -> io::Result<()> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "rvt: needs <image>",
        ));
    }
    rvt::parse_file(&args[0])
}

fn unpack_cmd(args: &[String]) -> io::Result<()> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unpack: needs <image>",
        ));
    }
    let image_path = &args[0];
    let frame = HvbFrame::load(image_path)?;

    print_frame_summary(&frame);

    let payload = frame.extract_image_payload();
    if payload.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty payload"));
    }
    fs::write("ramdisk.bin", payload)?;
    eprintln!(
        "Wrote ramdisk.bin ({} bytes, before decompression)",
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
    fs::write("ramdisk.cpio", &cpio_bytes)?;
    eprintln!(
        "Wrote ramdisk.cpio ({} bytes, decompressed)",
        cpio_bytes.len()
    );

    let hdr_json = frame_to_json(&frame);
    fs::write("header.json", hdr_json)?;
    eprintln!("Wrote header.json");
    Ok(())
}

fn repack_cmd(args: &[String]) -> io::Result<()> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "repack: needs <orig_image>",
        ));
    }
    let orig_path = &args[0];
    let out_path = if args.len() >= 2 {
        args[1].clone()
    } else {
        "new-ramdisk.img".to_string()
    };

    let mut frame = HvbFrame::load(orig_path)?;
    let orig_payload = frame.extract_image_payload().to_vec();

    // Read ramdisk.cpio from current dir
    let cpio_bytes = fs::read("ramdisk.cpio")?;
    eprintln!("Read ramdisk.cpio ({} bytes)", cpio_bytes.len());

    rebuild_image(&mut frame, &orig_payload, &cpio_bytes, &out_path)
}

fn cpio_cmd(args: &[String]) -> io::Result<()> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cpio: needs <incpio> [cmds...]",
        ));
    }
    let file = &args[0];
    let cmds: Vec<String> = args[1..].to_vec();

    let mut cpio = if Path::new(file).exists() {
        Cpio::load_from_file(file)?
    } else {
        Cpio::new()
    };

    for cmd in &cmds {
        if cmd.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        match parts[0] {
            "test" => {
                if cpio.exists(".backup/init_early") {
                    std::process::exit(1);
                }
                if cpio.exists("bin/init_early") || cpio.exists("init") {
                    std::process::exit(0);
                }
                std::process::exit(2);
            }
            "exists" => {
                if parts.len() < 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "exists: needs ENTRY",
                    ));
                }
                if cpio.exists(parts[1]) {
                    std::process::exit(0);
                } else {
                    std::process::exit(1);
                }
            }
            "ls" => {
                let recursive = parts.contains(&"-r");
                let positional: Vec<&str> = parts
                    .iter()
                    .skip(1)
                    .filter(|p| **p != "-r")
                    .copied()
                    .collect();
                let path = positional.first().copied().unwrap_or("/");
                cpio.ls(path, recursive);
            }
            "rm" => {
                let recursive = parts.contains(&"-r");
                let positional: Vec<&str> = parts
                    .iter()
                    .skip(1)
                    .filter(|p| **p != "-r")
                    .copied()
                    .collect();
                let path = positional.first().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "rm: needs ENTRY")
                })?;
                cpio.rm(path, recursive);
            }
            "mkdir" => {
                if parts.len() < 3 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "mkdir: needs MODE ENTRY",
                    ));
                }
                let mode = u32::from_str_radix(parts[1], 8)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "mkdir: bad mode"))?;
                cpio.mkdir(mode, parts[2]);
            }
            "ln" => {
                if parts.len() < 3 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "ln: needs SRC DST",
                    ));
                }
                cpio.ln(parts[1], parts[2]);
            }
            "mv" => {
                if parts.len() < 3 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "mv: needs SRC DST",
                    ));
                }
                cpio.mv(parts[1], parts[2])?;
            }
            "add" => {
                if parts.len() < 4 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "add: needs MODE ENTRY INFILE",
                    ));
                }
                let mode = u32::from_str_radix(parts[1], 8)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "add: bad mode"))?;
                cpio.add(mode, parts[2], parts[3])?;
            }
            "extract" => {
                if parts.len() == 1 {
                    cpio.extract(&[])?;
                } else if parts.len() == 3 {
                    cpio.extract(&[parts[1], parts[2]])?;
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "extract: needs 0 or 2 args",
                    ));
                }
                return Ok(());
            }
            other => {
                eprintln!("WARN: unknown cpio command '{other}', skipped");
            }
        }
    }
    cpio.dump(file)?;
    Ok(())
}

fn info_cmd(args: &[String]) -> io::Result<()> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "info: needs <image>",
        ));
    }
    let frame = HvbFrame::load(&args[0])?;
    print_frame_summary(&frame);
    Ok(())
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
