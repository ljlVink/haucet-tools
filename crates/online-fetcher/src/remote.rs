use anyhow::{Context, Result, ensure};
use reqwest::blocking::{Client, Response};
use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, ETAG, HeaderName,
    HeaderValue, IF_RANGE, LAST_MODIFIED, RANGE,
};
use reqwest::{StatusCode, Url};
use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

const MAX_TRANSFER: u64 = 8 * 1024 * 1024;
const MAX_REQUESTS: usize = 512;
const MAX_READ: u64 = 64 * 1024;
const READ_AHEAD: u64 = 512;

struct CachedRange {
    start: u64,
    bytes: Vec<u8>,
}

pub(crate) struct RemoteFile {
    client: Client,
    url: Url,
    length: u64,
    position: u64,
    validator: Option<(HeaderName, HeaderValue)>,
    cache: Vec<CachedRange>,
    downloaded: u64,
    requests: usize,
    failure: Option<String>,
}

impl RemoteFile {
    pub(crate) fn open(url: &str) -> Result<Self> {
        let url = Url::parse(url).context("invalid archive URL")?;
        ensure!(
            matches!(url.scheme(), "http" | "https"),
            "archive URL must use http:// or https://"
        );
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!("haucet-online-fetcher/", env!("CARGO_PKG_VERSION")))
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .context("creating HTTP client")?;
        // Some firmware CDNs reject HEAD even though ranged GET works.
        let response = client
            .get(url)
            .header(ACCEPT_ENCODING, "identity")
            .header(RANGE, "bytes=0-0")
            .send()
            .context("requesting archive size with HTTP Range")?;
        let length = validate_response(&response, 0, 0, None)?;
        let url = response.url().clone();
        let validator = response
            .headers()
            .get(ETAG)
            .filter(|value| !value.as_bytes().starts_with(b"W/"))
            .map(|value| (ETAG, value.clone()))
            .or_else(|| {
                response
                    .headers()
                    .get(LAST_MODIFIED)
                    .map(|value| (LAST_MODIFIED, value.clone()))
            });
        let bytes = read_body(response, 1)?;
        ensure!(length >= 22, "remote file is too small to be a ZIP archive");
        Ok(Self {
            client,
            url,
            length,
            position: 0,
            validator,
            cache: vec![CachedRange { start: 0, bytes }],
            downloaded: 1,
            requests: 1,
            failure: None,
        })
    }

    pub(crate) fn url(&self) -> &str {
        self.url.as_str()
    }

    pub(crate) fn len(&self) -> u64 {
        self.length
    }

    pub(crate) fn downloaded_bytes(&self) -> u64 {
        self.downloaded
    }

    pub(crate) fn requests(&self) -> usize {
        self.requests
    }

    pub(crate) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn read_range(&mut self, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() || self.position >= self.length {
            return Ok(0);
        }
        if let Some(reason) = &self.failure {
            anyhow::bail!("{reason}");
        }
        if let Some(cached) = self.cache.iter().find(|cached| {
            self.position >= cached.start
                && self.position - cached.start < cached.bytes.len() as u64
        }) {
            let offset = (self.position - cached.start) as usize;
            let count = buffer.len().min(cached.bytes.len() - offset);
            buffer[..count].copy_from_slice(&cached.bytes[offset..offset + count]);
            self.position += count as u64;
            return Ok(count);
        }

        let next_cached = self
            .cache
            .iter()
            .filter(|cached| cached.start > self.position)
            .map(|cached| cached.start)
            .min()
            .unwrap_or(self.length);
        let count = (buffer.len() as u64)
            .clamp(READ_AHEAD, MAX_READ)
            .min(next_cached - self.position);
        ensure!(
            self.requests < MAX_REQUESTS,
            "HTTP Range request limit exceeded"
        );
        ensure!(
            self.downloaded + count <= MAX_TRANSFER,
            "remote ZIP exceeds the 8 MiB transfer limit"
        );
        ensure!(
            self.downloaded + count < self.length,
            "reading this ZIP would require downloading the entire archive"
        );
        let end = self.position + count - 1;
        let mut request = self
            .client
            .get(self.url.clone())
            .header(ACCEPT_ENCODING, "identity")
            .header(RANGE, format!("bytes={}-{}", self.position, end));
        if let Some((_, validator)) = &self.validator {
            request = request.header(IF_RANGE, validator);
        }
        self.requests += 1;
        let response = request.send().context("reading HTTP byte range")?;
        validate_response(&response, self.position, end, Some(self.length))?;
        if let Some((name, validator)) = &self.validator
            && let Some(current) = response.headers().get(name)
        {
            ensure!(current == validator, "remote archive changed while reading");
        }
        let bytes = read_body(response, count)?;
        self.downloaded += bytes.len() as u64;
        self.cache.push(CachedRange {
            start: self.position,
            bytes,
        });
        self.read_range(buffer)
    }
}

impl Read for RemoteFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.read_range(buffer).map_err(|error| {
            let message = format!("{error:#}");
            self.failure = Some(message.clone());
            io::Error::other(message)
        })
    }
}

impl Seek for RemoteFile {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::End(offset) => self.length.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid remote ZIP seek"))?;
        self.position = position;
        Ok(position)
    }
}

fn validate_response(response: &Response, start: u64, end: u64, total: Option<u64>) -> Result<u64> {
    ensure!(
        response.status() == StatusCode::PARTIAL_CONTENT,
        "server must return HTTP 206 for byte ranges (received {}); whole-file downloads are disabled",
        response.status()
    );
    if let Some(encoding) = response.headers().get(CONTENT_ENCODING) {
        ensure!(
            encoding == "identity",
            "encoded HTTP range responses are unsupported"
        );
    }
    let header = response
        .headers()
        .get(CONTENT_RANGE)
        .context("HTTP 206 response is missing Content-Range")?
        .to_str()
        .context("invalid Content-Range header")?;
    let (range, length) = header
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'))
        .context("invalid Content-Range header")?;
    let (actual_start, actual_end) = range
        .split_once('-')
        .context("invalid Content-Range bounds")?;
    let actual_start = actual_start.parse::<u64>().context("invalid range start")?;
    let actual_end = actual_end.parse::<u64>().context("invalid range end")?;
    let length = length
        .parse::<u64>()
        .context("missing or invalid archive size")?;
    ensure!(
        actual_start == start && actual_end == end && end < length,
        "server returned a different byte range than requested"
    );
    ensure!(
        total.is_none_or(|total| total == length),
        "remote archive changed size while reading"
    );
    if let Some(content_length) = response.headers().get(CONTENT_LENGTH) {
        ensure!(
            content_length.to_str()?.parse::<u64>()? == end - start + 1,
            "HTTP range Content-Length does not match the requested range"
        );
    }
    Ok(length)
}

fn read_body(response: Response, expected: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .take(expected + 1)
        .read_to_end(&mut bytes)
        .context("reading HTTP range body")?;
    ensure!(
        bytes.len() as u64 == expected,
        "HTTP range body has an unexpected length"
    );
    Ok(bytes)
}
