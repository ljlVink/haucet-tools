use crate::format::FileFormat;
use bzip2::Compression as BzCompression;
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;
use flate2::Compression as GzCompression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use lz4::block::CompressionMode;
use lz4::liblz4::BlockChecksum;
use lz4::{
    BlockMode, BlockSize, ContentChecksum, Decoder as LZ4FrameDecoder, Encoder as LZ4FrameEncoder,
    EncoderBuilder as LZ4FrameEncoderBuilder,
};
use lzma_rust2::LZMAReader;
use std::cmp::min;
use std::io::{Cursor, Read, Result, Write};

const LZ4_BLOCK_SIZE: usize = 0x800000;
const LZ4HC_CLEVEL_MAX: i32 = 12;
const LZ4_MAGIC: u32 = 0x184c2102;

pub trait WriteFinish<W: Write>: Write {
    fn finish(self: Box<Self>) -> Result<W>;
}

macro_rules! finish_impl {
    ($($t:ty),*) => {$(
        impl<W: Write> WriteFinish<W> for $t {
            fn finish(self: Box<Self>) -> Result<W> {
                Self::finish(*self)
            }
        }
    )*}
}

finish_impl!(GzEncoder<W>, BzEncoder<W>);

impl<W: Write> WriteFinish<W> for LZ4FrameEncoder<W> {
    fn finish(self: Box<Self>) -> Result<W> {
        let (w, r) = Self::finish(*self);
        r?;
        Ok(w)
    }
}

struct LZ4BlockEncoder<W: Write> {
    write: W,
    buf: Vec<u8>,
    out_buf: Vec<u8>,
    total: u32,
    is_lg: bool,
    started: bool,
}

impl<W: Write> LZ4BlockEncoder<W> {
    fn new(write: W, is_lg: bool) -> Self {
        let cap = lz4::block::compress_bound(LZ4_BLOCK_SIZE).unwrap_or(LZ4_BLOCK_SIZE);
        LZ4BlockEncoder {
            write,
            buf: Vec::with_capacity(LZ4_BLOCK_SIZE),
            out_buf: vec![0u8; cap],
            total: 0,
            is_lg,
            started: false,
        }
    }

    fn flush_block(&mut self) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        if !self.started {
            self.write.write_all(&LZ4_MAGIC.to_le_bytes())?;
            self.started = true;
        }
        let compressed = lz4::block::compress_to_buffer(
            &self.buf,
            Some(CompressionMode::HIGHCOMPRESSION(LZ4HC_CLEVEL_MAX)),
            false,
            &mut self.out_buf,
        )?;
        let block_size = compressed as u32;
        self.write.write_all(&block_size.to_le_bytes())?;
        self.write.write_all(&self.out_buf[..compressed])?;
        self.buf.clear();
        Ok(())
    }
}

impl<W: Write> Write for LZ4BlockEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }

    fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        self.total = self.total.saturating_add(buf.len() as u32);
        while !buf.is_empty() {
            let room = LZ4_BLOCK_SIZE - self.buf.len();
            let take = min(room, buf.len());
            self.buf.extend_from_slice(&buf[..take]);
            buf = &buf[take..];
            if self.buf.len() == LZ4_BLOCK_SIZE {
                self.flush_block()?;
            }
        }
        Ok(())
    }
}

impl<W: Write> WriteFinish<W> for LZ4BlockEncoder<W> {
    fn finish(mut self: Box<Self>) -> Result<W> {
        self.flush_block()?;
        if self.is_lg {
            self.write.write_all(&self.total.to_le_bytes())?;
        }
        Ok(self.write)
    }
}

struct LZ4BlockDecoder<R: Read> {
    read: R,
    in_buf: Vec<u8>,
    out_buf: Vec<u8>,
    out_len: usize,
    out_pos: usize,
}

impl<R: Read> LZ4BlockDecoder<R> {
    fn new(read: R) -> Self {
        let cap = lz4::block::compress_bound(LZ4_BLOCK_SIZE).unwrap_or(LZ4_BLOCK_SIZE);
        Self {
            read,
            in_buf: vec![0u8; cap],
            out_buf: vec![0u8; LZ4_BLOCK_SIZE],
            out_len: 0,
            out_pos: 0,
        }
    }
}

impl<R: Read> Read for LZ4BlockDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.out_pos == self.out_len {
            let mut block_size_buf = [0u8; 4];
            match self.read.read_exact(&mut block_size_buf) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(0),
                Err(e) => return Err(e),
            }
            let mut block_size = u32::from_le_bytes(block_size_buf);
            if block_size == LZ4_MAGIC {
                self.read.read_exact(&mut block_size_buf)?;
                block_size = u32::from_le_bytes(block_size_buf);
            }
            let block_size = block_size as usize;
            if block_size > self.in_buf.len() {
                return Ok(0);
            }
            self.read.read_exact(&mut self.in_buf[..block_size])?;
            self.out_len = lz4::block::decompress_to_buffer(
                &self.in_buf[..block_size],
                Some(LZ4_BLOCK_SIZE as i32),
                &mut self.out_buf,
            )?;
            self.out_pos = 0;
        }
        let copy_len = min(buf.len(), self.out_len - self.out_pos);
        buf[..copy_len].copy_from_slice(&self.out_buf[self.out_pos..self.out_pos + copy_len]);
        self.out_pos += copy_len;
        Ok(copy_len)
    }
}

pub fn get_encoder<'a, W: Write + 'a>(
    format: FileFormat,
    w: W,
) -> Result<Box<dyn WriteFinish<W> + 'a>> {
    Ok(match format {
        FileFormat::BZIP2 => Box::new(BzEncoder::new(w, BzCompression::best())),
        FileFormat::LZ4 => {
            let encoder = LZ4FrameEncoderBuilder::new()
                .block_size(BlockSize::Max4MB)
                .block_mode(BlockMode::Independent)
                .checksum(ContentChecksum::ChecksumEnabled)
                .block_checksum(BlockChecksum::BlockChecksumEnabled)
                .level(9)
                .auto_flush(true)
                .build(w)?;
            Box::new(encoder)
        }
        FileFormat::LZ4_LEGACY => Box::new(LZ4BlockEncoder::new(w, false)),
        FileFormat::LZ4_LG => Box::new(LZ4BlockEncoder::new(w, true)),
        FileFormat::GZIP | FileFormat::ZOPFLI => Box::new(GzEncoder::new(w, GzCompression::best())),
        FileFormat::LZMA => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "LZMA encoder not supported in this build; use gzip/lz4/raw",
            ));
        }
        FileFormat::XZ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "XZ not supported in this build (no xz-rust crate); use gzip/lz4/raw",
            ));
        }
        _ => unreachable!("format {format:?} is not a compression"),
    })
}

pub fn get_decoder<'a, R: Read + 'a>(format: FileFormat, r: R) -> Result<Box<dyn Read + 'a>> {
    Ok(match format {
        FileFormat::BZIP2 => Box::new(BzDecoder::new(r)),
        FileFormat::LZ4 => Box::new(LZ4FrameDecoder::new(r)?),
        FileFormat::LZ4_LG | FileFormat::LZ4_LEGACY => Box::new(LZ4BlockDecoder::new(r)),
        FileFormat::GZIP | FileFormat::ZOPFLI => Box::new(GzDecoder::new(r)),
        FileFormat::LZMA => Box::new(LZMAReader::new_mem_limit(r, u32::MAX, None)?),
        FileFormat::XZ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "XZ not supported in this build",
            ));
        }
        _ => unreachable!("format {format:?} is not a compression"),
    })
}

pub fn compress_vec(format: FileFormat, in_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = get_encoder(format, Vec::<u8>::new())?;
    encoder.write_all(in_bytes)?;
    encoder.finish()
}

pub fn decompress_vec(format: FileFormat, in_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = get_decoder(format, Cursor::new(in_bytes))?;
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}
