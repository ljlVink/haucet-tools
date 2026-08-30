use crate::formats::header::FileFormat;
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
    total: u64,
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
        self.total = self
            .total
            .checked_add(buf.len() as u64)
            .ok_or_else(|| std::io::Error::other("LZ4 input size overflow"))?;
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
            let total = u32::try_from(self.total).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "LZ4_LG input exceeds the 4 GiB format limit",
                )
            })?;
            self.write.write_all(&total.to_le_bytes())?;
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
    expected_size: Option<u32>,
    total_out: u64,
    finished: bool,
}

impl<R: Read> LZ4BlockDecoder<R> {
    fn new(read: R) -> Self {
        Self::with_expected_size(read, None)
    }

    fn with_expected_size(read: R, expected_size: Option<u32>) -> Self {
        let cap = lz4::block::compress_bound(LZ4_BLOCK_SIZE).unwrap_or(LZ4_BLOCK_SIZE);
        Self {
            read,
            in_buf: vec![0u8; cap],
            out_buf: vec![0u8; LZ4_BLOCK_SIZE],
            out_len: 0,
            out_pos: 0,
            expected_size,
            total_out: 0,
            finished: false,
        }
    }

    fn finish_stream(&mut self) -> Result<usize> {
        if !self.finished {
            self.finished = true;
            if let Some(expected) = self.expected_size
                && self.total_out != u64::from(expected)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "LZ4_LG size trailer says {expected} bytes, decoded {}",
                        self.total_out
                    ),
                ));
            }
        }
        Ok(0)
    }
}

impl<R: Read> Read for LZ4BlockDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.finished {
            return Ok(0);
        }
        if self.out_pos == self.out_len {
            let mut block_size_buf = [0u8; 4];
            let first = self.read.read(&mut block_size_buf[..1])?;
            if first == 0 {
                return self.finish_stream();
            }
            self.read.read_exact(&mut block_size_buf[1..])?;
            let mut block_size = u32::from_le_bytes(block_size_buf);
            if block_size == LZ4_MAGIC {
                self.read.read_exact(&mut block_size_buf)?;
                block_size = u32::from_le_bytes(block_size_buf);
            }
            let block_size = block_size as usize;
            if block_size == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "LZ4 block is empty",
                ));
            }
            if block_size > self.in_buf.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("LZ4 block is too large: {block_size} bytes"),
                ));
            }
            self.read.read_exact(&mut self.in_buf[..block_size])?;
            self.out_len = lz4::block::decompress_to_buffer(
                &self.in_buf[..block_size],
                Some(LZ4_BLOCK_SIZE as i32),
                &mut self.out_buf,
            )?;
            if self.out_len == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "LZ4 block decoded to no data",
                ));
            }
            self.out_pos = 0;
        }
        let copy_len = min(buf.len(), self.out_len - self.out_pos);
        buf[..copy_len].copy_from_slice(&self.out_buf[self.out_pos..self.out_pos + copy_len]);
        self.out_pos += copy_len;
        self.total_out = self
            .total_out
            .checked_add(copy_len as u64)
            .ok_or_else(|| std::io::Error::other("decoded LZ4 size overflow"))?;
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
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("format {other:?} is not a compression"),
            ));
        }
    })
}

pub fn get_decoder<'a, R: Read + 'a>(format: FileFormat, r: R) -> Result<Box<dyn Read + 'a>> {
    Ok(match format {
        FileFormat::BZIP2 => Box::new(BzDecoder::new(r)),
        FileFormat::LZ4 => Box::new(LZ4FrameDecoder::new(r)?),
        FileFormat::LZ4_LEGACY => Box::new(LZ4BlockDecoder::new(r)),
        FileFormat::LZ4_LG => {
            let mut encoded = Vec::new();
            let mut r = r;
            r.read_to_end(&mut encoded)?;
            if encoded.len() < 4 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "LZ4_LG stream is missing its size trailer",
                ));
            }
            let trailer = encoded.split_off(encoded.len() - 4);
            let expected_size = u32::from_le_bytes(trailer.try_into().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid LZ4_LG trailer")
            })?);
            Box::new(LZ4BlockDecoder::with_expected_size(
                Cursor::new(encoded),
                Some(expected_size),
            ))
        }
        FileFormat::GZIP | FileFormat::ZOPFLI => Box::new(GzDecoder::new(r)),
        FileFormat::LZMA => Box::new(LZMAReader::new_mem_limit(r, u32::MAX, None)?),
        FileFormat::XZ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "XZ not supported in this build",
            ));
        }
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("format {other:?} is not a compression"),
            ));
        }
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
