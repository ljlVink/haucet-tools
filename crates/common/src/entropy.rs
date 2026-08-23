use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

const BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropySummary {
    pub size: u64,
    pub entropy_bits_per_byte: f64,
    pub normalized: f64,
    pub unique_bytes: usize,
    pub most_common: Option<ByteFrequency>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteFrequency {
    pub byte: u8,
    pub count: u64,
    pub ratio: f64,
}

impl EntropySummary {
    pub fn normalized_percent(&self) -> f64 {
        self.normalized * 100.0
    }
}

pub fn analyze_file(path: &Path) -> Result<EntropySummary> {
    let file = File::open(path).with_context(|| format!("打开 {}", path.display()))?;
    analyze_reader(BufReader::with_capacity(BUFFER_SIZE, file))
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

    Ok(summarize_counts(total, &counts))
}

fn summarize_counts(total: u64, counts: &[u64; 256]) -> EntropySummary {
    if total == 0 {
        return EntropySummary {
            size: 0,
            entropy_bits_per_byte: 0.0,
            normalized: 0.0,
            unique_bytes: 0,
            most_common: None,
        };
    }

    let total_f = total as f64;
    let mut entropy = 0.0_f64;
    let mut unique_bytes = 0_usize;
    let mut most_common = ByteFrequency {
        byte: 0,
        count: 0,
        ratio: 0.0,
    };

    for (byte, count) in counts.iter().copied().enumerate() {
        if count == 0 {
            continue;
        }
        unique_bytes += 1;
        let probability = count as f64 / total_f;
        entropy -= probability * probability.log2();
        if count > most_common.count {
            most_common = ByteFrequency {
                byte: byte as u8,
                count,
                ratio: probability,
            };
        }
    }

    EntropySummary {
        size: total,
        entropy_bits_per_byte: entropy,
        normalized: entropy / 8.0,
        unique_bytes,
        most_common: Some(most_common),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn empty_input_has_zero_entropy() {
        let summary = analyze_reader(Cursor::new(Vec::<u8>::new())).unwrap();
        assert_eq!(summary.size, 0);
        assert_eq!(summary.entropy_bits_per_byte, 0.0);
        assert_eq!(summary.unique_bytes, 0);
        assert!(summary.most_common.is_none());
    }

    #[test]
    fn repeated_byte_has_zero_entropy() {
        let summary = analyze_reader(Cursor::new(vec![0x41; 1024])).unwrap();
        assert_eq!(summary.size, 1024);
        assert_eq!(summary.entropy_bits_per_byte, 0.0);
        assert_eq!(summary.unique_bytes, 1);
        assert_eq!(summary.most_common.unwrap().byte, 0x41);
    }

    #[test]
    fn balanced_all_byte_values_has_full_entropy() {
        let data = (0_u8..=255).cycle().take(256 * 8).collect::<Vec<_>>();
        let summary = analyze_reader(Cursor::new(data)).unwrap();
        assert_eq!(summary.unique_bytes, 256);
        assert!((summary.entropy_bits_per_byte - 8.0).abs() < f64::EPSILON);
        assert!((summary.normalized - 1.0).abs() < f64::EPSILON);
    }
}
