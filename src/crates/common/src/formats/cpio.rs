use super::header::FileFormat;
use crate::compress::{get_decoder, get_encoder};
use bytemuck::{Pod, Zeroable, from_bytes};
mod test_helper_unused {
    #[allow(dead_code)]
    pub fn _ord() {}
}
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{Read, Write};
use std::mem::size_of;
use std::path::PathBuf;

#[derive(Copy, Clone, Pod, Zeroable)]
#[repr(C, packed)]
struct CpioHeader {
    magic: [u8; 6],
    ino: [u8; 8],
    mode: [u8; 8],
    uid: [u8; 8],
    gid: [u8; 8],
    nlink: [u8; 8],
    mtime: [u8; 8],
    filesize: [u8; 8],
    devmajor: [u8; 8],
    devminor: [u8; 8],
    rdevmajor: [u8; 8],
    rdevminor: [u8; 8],
    namesize: [u8; 8],
    check: [u8; 8],
}

pub struct Cpio {
    pub entries: BTreeMap<String, CpioEntry>,
}

#[derive(Clone)]
pub struct CpioEntry {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdevmajor: u32,
    pub rdevminor: u32,
    pub data: Vec<u8>,
}

pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IRUSR: u32 = 0o400;
pub const S_IWUSR: u32 = 0o200;
pub const S_IXUSR: u32 = 0o100;
pub const S_IRGRP: u32 = 0o040;
pub const S_IWGRP: u32 = 0o020;
pub const S_IXGRP: u32 = 0o010;
pub const S_IROTH: u32 = 0o004;
pub const S_IWOTH: u32 = 0o002;
pub const S_IXOTH: u32 = 0o001;

pub fn parse_cpio_mode(mode: &str) -> std::io::Result<u32> {
    u32::from_str_radix(mode, 8).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid cpio mode: {mode}"),
        )
    })
}

impl Cpio {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn load_from_data(data: &[u8]) -> std::io::Result<Self> {
        let mut cpio = Cpio::new();
        let mut pos = 0_usize;
        while pos < data.len() {
            let hdr_sz = size_of::<CpioHeader>();
            if pos + hdr_sz > data.len() {
                break;
            }
            let hdr = from_bytes::<CpioHeader>(&data[pos..(pos + hdr_sz)]);
            if &hdr.magic != b"070701" && &hdr.magic != b"070702" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid cpio magic",
                ));
            }
            pos += hdr_sz;
            let name_sz = x8u(&hdr.namesize)? as usize;
            if pos + name_sz > data.len() {
                break;
            }
            let name_end = pos + name_sz;
            let name = std::str::from_utf8(&data[pos..name_end])
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-utf8 name"))?
                .trim_end_matches('\0')
                .to_string();
            pos = align_4(name_end);
            if name == "." || name == ".." {
                continue;
            }
            if name == "TRAILER!!!" {
                match data[pos..].windows(6).position(|w| w == b"070701") {
                    Some(x) => pos += x,
                    None => break,
                }
                continue;
            }
            let file_sz = x8u(&hdr.filesize)? as usize;
            let data_end = pos + file_sz;
            if data_end > data.len() {
                break;
            }
            cpio.entries.insert(
                name.clone(),
                CpioEntry {
                    mode: x8u(&hdr.mode)?,
                    uid: x8u(&hdr.uid)?,
                    gid: x8u(&hdr.gid)?,
                    rdevmajor: x8u(&hdr.rdevmajor)?,
                    rdevminor: x8u(&hdr.rdevminor)?,
                    data: data[pos..data_end].to_vec(),
                },
            );
            pos = align_4(data_end);
        }
        Ok(cpio)
    }

    pub fn load_from_file(path: &str) -> std::io::Result<Self> {
        eprintln!("Loading cpio: [{path}]");
        let data = fs::read(path)?;
        Self::load_from_data(&data)
    }

    pub fn dump(&self, path: &str) -> std::io::Result<()> {
        eprintln!("Dumping cpio: [{path}]");
        let mut buf = Vec::new();
        self.dump_to(&mut buf)?;
        fs::write(path, buf)?;
        Ok(())
    }

    pub fn dump_to(&self, out: &mut Vec<u8>) -> std::io::Result<()> {
        let mut pos = 0usize;
        let mut inode = 300000u32;
        for (name, entry) in &self.entries {
            let header = format!(
                "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
                inode,
                entry.mode,
                entry.uid,
                entry.gid,
                1u32,
                0u32,
                entry.data.len(),
                0u32,
                0u32,
                entry.rdevmajor,
                entry.rdevminor,
                name.len() + 1,
                0u32,
            );
            out.extend_from_slice(header.as_bytes());
            pos += header.len();
            out.extend_from_slice(name.as_bytes());
            pos += name.len();
            out.push(0);
            pos += 1;
            let pad = align_4(pos) - pos;
            out.extend_from_slice(&vec![0u8; pad]);
            pos = align_4(pos);
            out.extend_from_slice(&entry.data);
            pos += entry.data.len();
            let pad = align_4(pos) - pos;
            out.extend_from_slice(&vec![0u8; pad]);
            pos = align_4(pos);
            inode += 1;
        }
        let trailer = format!(
            "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
            inode, 0o755u32, 0u32, 0u32, 1u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 11u32, 0u32
        );
        out.extend_from_slice(trailer.as_bytes());
        pos += trailer.len();
        out.extend_from_slice(b"TRAILER!!!\0");
        pos += "TRAILER!!!\0".len();
        let pad = align_4(pos) - pos;
        out.extend_from_slice(&vec![0u8; pad]);
        Ok(())
    }

    pub fn rm(&mut self, path: &str, recursive: bool) {
        let path = norm_path(path);
        if self.entries.remove(&path).is_some() {
            eprintln!("Removed entry [{path}]");
        }
        if recursive {
            let path = path + "/";
            self.entries.retain(|k, _| {
                if k.starts_with(&path) {
                    eprintln!("Removed entry [{k}]");
                    false
                } else {
                    true
                }
            });
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.entries.contains_key(&norm_path(path))
    }

    pub fn mkdir(&mut self, mode: u32, dir: &str) {
        self.entries.insert(
            norm_path(dir),
            CpioEntry {
                mode: mode | S_IFDIR,
                uid: 0,
                gid: 0,
                rdevmajor: 0,
                rdevminor: 0,
                data: vec![],
            },
        );
        eprintln!("Create directory [{dir}] ({mode:04o})");
    }

    pub fn ln(&mut self, src: &str, dst: &str) {
        self.entries.insert(
            norm_path(dst),
            CpioEntry {
                mode: S_IFLNK,
                uid: 0,
                gid: 0,
                rdevmajor: 0,
                rdevminor: 0,
                data: norm_path(src).as_bytes().to_vec(),
            },
        );
        eprintln!("Create symlink [{dst}] -> [{src}]");
    }

    pub fn mv(&mut self, from: &str, to: &str) -> std::io::Result<()> {
        let entry = self.entries.remove(&norm_path(from)).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no such entry {from}"),
            )
        })?;
        self.entries.insert(norm_path(to), entry);
        eprintln!("Move [{from}] -> [{to}]");
        Ok(())
    }

    pub fn add_buf(&mut self, mode: u32, path: &str, data: &[u8]) {
        let mode = S_IFREG | (mode & 0o7777);
        self.entries.insert(
            norm_path(path),
            CpioEntry {
                mode,
                uid: 0,
                gid: 0,
                rdevmajor: 0,
                rdevminor: 0,
                data: data.to_vec(),
            },
        );
        eprintln!("Add file [{path}] ({mode:04o})");
    }

    pub fn extract_entry(&self, path: &str, out_path: &str) -> std::io::Result<()> {
        let entry = self.entries.get(&norm_path(path)).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no such entry {path}"),
            )
        })?;
        eprintln!("Extracting entry [{path}] -> [{out_path}]");
        let p = PathBuf::from(out_path);
        if let Some(parent) = p.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mode = entry.mode & 0o7777;
        match entry.mode & S_IFMT {
            S_IFDIR => {
                fs::create_dir_all(&p)?;
                set_mode(&p, mode)?;
            }
            S_IFREG => {
                fs::write(&p, &entry.data)?;
                set_mode(&p, mode)?;
            }
            S_IFLNK => {
                let target = std::str::from_utf8(&entry.data).unwrap_or("");
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(target, &p)?;
                }
                #[cfg(not(unix))]
                {
                    eprintln!("WARN: symlink {path} -> {target} skipped (host not unix)");
                }
            }
            S_IFBLK | S_IFCHR => {
                eprintln!(
                    "WARN: device node {path} (major={}, minor={}) skipped",
                    entry.rdevmajor, entry.rdevminor
                );
            }
            _ => eprintln!(
                "WARN: unknown entry type for {path} (mode={:o})",
                entry.mode
            ),
        }
        Ok(())
    }

    pub fn extract(&self, paths: &[&str]) -> std::io::Result<()> {
        if paths.is_empty() {
            for path in self.entries.keys() {
                if path == "." || path == ".." {
                    continue;
                }
                self.extract_entry(path, path)?;
            }
        } else if paths.len() == 2 {
            self.extract_entry(paths[0], paths[1])?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "extract needs 0 or 2 args",
            ));
        }
        Ok(())
    }

    pub fn add(&mut self, mode: u32, path: &str, file: &str) -> std::io::Result<()> {
        if path.ends_with('/') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path cannot end with / for add",
            ));
        }
        let meta = fs::symlink_metadata(file)?;
        let is_symlink = meta.file_type().is_symlink();
        if meta.is_dir() {
            self.mkdir(mode, path);
            return Ok(());
        }
        let content = if is_symlink {
            #[cfg(unix)]
            {
                std::fs::read_link(file)?
                    .to_str()
                    .ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, "non-utf8 symlink")
                    })?
                    .as_bytes()
                    .to_vec()
            }
            #[cfg(not(unix))]
            {
                Vec::new()
            }
        } else {
            fs::read(file)?
        };
        let mode = if is_symlink { S_IFLNK } else { S_IFREG } | (mode & 0o7777);
        self.entries.insert(
            norm_path(path),
            CpioEntry {
                mode,
                uid: 0,
                gid: 0,
                rdevmajor: 0,
                rdevminor: 0,
                data: content,
            },
        );
        eprintln!("Add file [{path}] ({mode:04o})");
        Ok(())
    }

    pub fn ls(&self, path: &str, recursive: bool) {
        let path = norm_path(path);
        let path = if path.is_empty() {
            path
        } else {
            format!("/{path}")
        };
        for (name, entry) in &self.entries {
            let p = format!("/{name}");
            let Some(p) = p.strip_prefix(path.as_str()) else {
                continue;
            };
            if !p.is_empty() && !p.starts_with('/') {
                continue;
            }
            if !recursive && !p.is_empty() && p.matches('/').count() > 1 {
                continue;
            }
            println!("{entry}\t{name}");
        }
    }
}

impl Default for Cpio {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for CpioEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let type_ch = match self.mode & S_IFMT {
            S_IFDIR => 'd',
            S_IFREG => '-',
            S_IFLNK => 'l',
            S_IFBLK => 'b',
            S_IFCHR => 'c',
            _ => '?',
        };
        let perm =
            |bit: u32, on: char, off: char| -> char { if self.mode & bit != 0 { on } else { off } };
        write!(
            f,
            "{}{}{}{}{}{}{}{}{}{}\t{}\t{}\t{}\t{}:{}",
            type_ch,
            perm(S_IRUSR, 'r', '-'),
            perm(S_IWUSR, 'w', '-'),
            perm(S_IXUSR, 'x', '-'),
            perm(S_IRGRP, 'r', '-'),
            perm(S_IWGRP, 'w', '-'),
            perm(S_IXGRP, 'x', '-'),
            perm(S_IROTH, 'r', '-'),
            perm(S_IWOTH, 'w', '-'),
            perm(S_IXOTH, 'x', '-'),
            self.uid,
            self.gid,
            self.data.len(),
            self.rdevmajor,
            self.rdevminor,
        )
    }
}

#[inline(always)]
fn align_4(x: usize) -> usize {
    (x + 3) & !3
}

#[inline(always)]
pub fn norm_path(path: &str) -> String {
    path.split('/')
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn x8u(x: &[u8; 8]) -> std::io::Result<u32> {
    let s = std::str::from_utf8(x)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad cpio header"))?;
    let mut ret = 0u32;
    for c in s.chars() {
        let v = c.to_digit(16).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "bad cpio header")
        })?;
        ret = ret * 16 + v;
    }
    Ok(ret)
}

#[cfg(unix)]
fn set_mode(p: &std::path::Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_p: &std::path::Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

impl CpioEntry {
    pub fn compress(&mut self) -> bool {
        if self.mode & S_IFMT != S_IFREG {
            return false;
        }
        let res: std::io::Result<Vec<u8>> = (|| {
            let mut enc = get_encoder(FileFormat::XZ, Vec::new())?;
            enc.write_all(&self.data)?;
            enc.finish()
        })();
        match res {
            Ok(data) => {
                self.data = data;
                true
            }
            Err(_) => {
                eprintln!("xz compression failed");
                false
            }
        }
    }

    pub fn decompress(&mut self) -> bool {
        if self.mode & S_IFMT != S_IFREG {
            return false;
        }
        let res: std::io::Result<Vec<u8>> = (|| {
            let mut dec = get_decoder(FileFormat::XZ, std::io::Cursor::new(&self.data))?;
            let mut data = Vec::new();
            dec.read_to_end(&mut data)?;
            Ok(data)
        })();
        match res {
            Ok(data) => {
                self.data = data;
                true
            }
            Err(_) => {
                eprintln!("xz decompression failed");
                false
            }
        }
    }
}
