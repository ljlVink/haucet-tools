use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

const BUFFER_SIZE: usize = 1024 * 1024;
const MIN_WINDOW_SIZE: u64 = 4 * 1024;
const MAX_WINDOW_SIZE: u64 = 1024 * 1024;
const MAX_WINDOW_POINTS: usize = 192;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropySummary {
    pub size: u64,
    pub entropy_bits_per_byte: f64,
    pub normalized: f64,
    pub unique_bytes: usize,
    pub most_common: Option<ByteFrequency>,
    pub window_size: u64,
    pub windows: Vec<EntropyWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteFrequency {
    pub byte: u8,
    pub count: u64,
    pub ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropyWindow {
    pub offset: u64,
    pub entropy_bits_per_byte: f64,
}

impl EntropySummary {
    pub fn normalized_percent(&self) -> f64 {
        self.normalized * 100.0
    }
}

pub fn analyze_file(path: &Path) -> Result<EntropySummary> {
    let file = File::open(path).with_context(|| format!("打开 {}", path.display()))?;
    let size = file
        .metadata()
        .with_context(|| format!("读取 {} 的大小", path.display()))?
        .len();
    analyze_reader_with_windows(BufReader::with_capacity(BUFFER_SIZE, file), size)
        .with_context(|| format!("计算 {} 的信息熵", path.display()))
}

pub fn analyze_reader(mut reader: impl Read) -> Result<EntropySummary> {
    let mut counts = [0_u64; 256];
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_SIZE];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        for byte in &buffer[..read] {
            counts[*byte as usize] += 1;
        }
    }

    Ok(summarize_counts(total, &counts, 0, Vec::new()))
}

fn analyze_reader_with_windows(
    mut reader: impl Read,
    expected_size: u64,
) -> Result<EntropySummary> {
    let (window_size, offsets) = window_plan(expected_size);
    let mut counts = [0_u64; 256];
    let mut window_counts = [0_u64; 256];
    let mut window_buffer = vec![0_u8; window_size as usize];
    let mut window_cursor = 0_usize;
    let mut window_filled = 0_usize;
    let mut windows = Vec::with_capacity(offsets.len());
    let mut next_window = 0_usize;
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_SIZE];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for &byte in &buffer[..read] {
            total += 1;
            counts[byte as usize] += 1;

            if window_size != 0 {
                if window_filled == window_buffer.len() {
                    let outgoing = window_buffer[window_cursor];
                    window_counts[outgoing as usize] -= 1;
                } else {
                    window_filled += 1;
                }
                window_buffer[window_cursor] = byte;
                window_counts[byte as usize] += 1;
                window_cursor += 1;
                if window_cursor == window_buffer.len() {
                    window_cursor = 0;
                }

                if next_window < offsets.len() && total == offsets[next_window] + window_size {
                    let (entropy, _) = entropy_and_unique(window_size, &window_counts);
                    windows.push(EntropyWindow {
                        offset: offsets[next_window],
                        entropy_bits_per_byte: entropy,
                    });
                    next_window += 1;
                }
            }
        }
    }

    Ok(summarize_counts(total, &counts, window_size, windows))
}

fn window_plan(size: u64) -> (u64, Vec<u64>) {
    if size == 0 {
        return (0, Vec::new());
    }
    let window_size = size
        .div_ceil(128)
        .clamp(MIN_WINDOW_SIZE, MAX_WINDOW_SIZE)
        .min(size);
    let span = size - window_size;
    if span == 0 {
        return (window_size, vec![0]);
    }
    let point_count = (span + 1).min(MAX_WINDOW_POINTS as u64) as usize;
    let denominator = (point_count - 1) as u128;
    let offsets = (0..point_count)
        .map(|index| ((index as u128 * span as u128) / denominator) as u64)
        .collect();
    (window_size, offsets)
}

fn entropy_and_unique(total: u64, counts: &[u64; 256]) -> (f64, usize) {
    if total == 0 {
        return (0.0, 0);
    }
    let total_f = total as f64;
    let mut entropy = 0.0_f64;
    let mut unique_bytes = 0_usize;
    for count in counts.iter().copied().filter(|count| *count != 0) {
        unique_bytes += 1;
        let probability = count as f64 / total_f;
        entropy -= probability * probability.log2();
    }
    (entropy, unique_bytes)
}

fn summarize_counts(
    total: u64,
    counts: &[u64; 256],
    window_size: u64,
    windows: Vec<EntropyWindow>,
) -> EntropySummary {
    if total == 0 {
        return EntropySummary {
            size: 0,
            entropy_bits_per_byte: 0.0,
            normalized: 0.0,
            unique_bytes: 0,
            most_common: None,
            window_size,
            windows,
        };
    }

    let (entropy, unique_bytes) = entropy_and_unique(total, counts);
    let mut most_common = ByteFrequency {
        byte: 0,
        count: 0,
        ratio: 0.0,
    };

    for (byte, count) in counts.iter().copied().enumerate() {
        if count == 0 {
            continue;
        }
        if count > most_common.count {
            most_common = ByteFrequency {
                byte: byte as u8,
                count,
                ratio: count as f64 / total as f64,
            };
        }
    }

    EntropySummary {
        size: total,
        entropy_bits_per_byte: entropy,
        normalized: entropy / 8.0,
        unique_bytes,
        most_common: Some(most_common),
        window_size,
        windows,
    }
}
