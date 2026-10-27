//! Pure local Codex observer helper library.
//!
//! Normalizes bounded Codex hook JSON from stdin into metadata-only [`NormalizedEvent`]
//! payloads and delivers them to the local loopback collector within a strict deadline.

use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use adapters::codex::{
    normalize_hook, CodexContext, Surface, MAX_RAW_HOOK_BYTES, SURFACE_STANDALONE_DESKTOP,
    SURFACE_VSCODE_EXTENSION,
};
use chrono::{DateTime, Utc};
use normalized_schema::{format_canonical_utc, validate_identifier, NormalizedEvent};
use serde::Deserialize;
use uuid::Uuid;

pub const MAX_CONFIG_BYTES: usize = 16 * 1024;
pub const CODEX_EVENTS_PATH: &str = "/events/codex";
pub const DEFAULT_MAIN_BUDGET: Duration = Duration::from_millis(200);
pub const MAX_ATTEMPTS: u8 = 2;

/// Sanitized helper outcome returned on successful loopback delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered { attempts: u8 },
}

/// Sanitized error categories. Never retains or formats raw config, tokens, or hook input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperError {
    InvalidCliArgs,
    ConfigIo,
    ConfigTooLarge,
    InvalidConfigJson,
    InvalidLoopbackAddress,
    InvalidIdentifier,
    InvalidSurface,
    InvalidToken,
    StdinIo,
    PayloadTooLarge,
    InvalidUtf8,
    Normalization,
    Timeout,
    Network,
    HttpStatus(u16),
}

/// Validated observer helper configuration.
///
/// Intentionally omits `Debug` and `Display` so bearer tokens and raw config values
/// are never formatted into logs or panic output.
#[derive(Clone, PartialEq, Eq)]
pub struct HelperConfig {
    address: SocketAddr,
    integration_id: String,
    account_key: String,
    surface: Surface,
    token: String,
}

impl HelperConfig {
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    #[must_use]
    pub fn integration_id(&self) -> &str {
        &self.integration_id
    }

    #[must_use]
    pub fn account_key(&self) -> &str {
        &self.account_key
    }

    #[must_use]
    pub const fn surface(&self) -> Surface {
        self.surface
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawHelperConfig {
    address: String,
    #[serde(alias = "integration_id")]
    integration_id: String,
    #[serde(alias = "account_key")]
    account_key: String,
    surface: String,
    token: String,
}

/// Parse CLI arguments expecting explicitly `--config <PATH>` or `--config=<PATH>`.
pub fn parse_cli_args<I, S>(args: I) -> Result<PathBuf, HelperError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let collected: Vec<OsString> = args.into_iter().map(Into::into).collect();
    match collected.as_slice() {
        [flag, path] if flag == "--config" && !path.is_empty() => Ok(PathBuf::from(path)),
        [single] => {
            let s = single.to_str().ok_or(HelperError::InvalidCliArgs)?;
            let rest = s
                .strip_prefix("--config=")
                .ok_or(HelperError::InvalidCliArgs)?;
            if rest.is_empty() {
                return Err(HelperError::InvalidCliArgs);
            }
            Ok(PathBuf::from(rest))
        }
        _ => Err(HelperError::InvalidCliArgs),
    }
}

/// Read and validate an observer helper configuration file (`<= 16 KiB`).
pub fn load_config(path: &Path) -> Result<HelperConfig, HelperError> {
    let file = File::open(path).map_err(|_| HelperError::ConfigIo)?;
    let mut buf = Vec::with_capacity(512);
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|_| HelperError::ConfigIo)?;
    if buf.len() > MAX_CONFIG_BYTES {
        return Err(HelperError::ConfigTooLarge);
    }
    let raw = std::str::from_utf8(&buf).map_err(|_| HelperError::InvalidConfigJson)?;
    parse_config(raw)
}

/// Parse and validate observer helper configuration JSON (`<= 16 KiB`).
pub fn parse_config(raw: &str) -> Result<HelperConfig, HelperError> {
    if raw.len() > MAX_CONFIG_BYTES {
        return Err(HelperError::ConfigTooLarge);
    }

    let parsed: RawHelperConfig =
        serde_json::from_str(raw).map_err(|_| HelperError::InvalidConfigJson)?;

    let address: SocketAddr = parsed
        .address
        .parse()
        .map_err(|_| HelperError::InvalidLoopbackAddress)?;
    match address {
        SocketAddr::V4(v4) if *v4.ip() == Ipv4Addr::LOCALHOST && v4.port() != 0 => {}
        _ => return Err(HelperError::InvalidLoopbackAddress),
    }

    validate_identifier("integrationId", &parsed.integration_id)
        .map_err(|_| HelperError::InvalidIdentifier)?;
    if !parsed.integration_id.is_ascii() {
        return Err(HelperError::InvalidIdentifier);
    }
    validate_identifier("accountKey", &parsed.account_key)
        .map_err(|_| HelperError::InvalidIdentifier)?;

    let surface = match parsed.surface.as_str() {
        SURFACE_VSCODE_EXTENSION => Surface::VsCodeExtension,
        SURFACE_STANDALONE_DESKTOP => Surface::StandaloneDesktop,
        _ => return Err(HelperError::InvalidSurface),
    };

    if parsed.token.len() != 64 || !parsed.token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(HelperError::InvalidToken);
    }

    Ok(HelperConfig {
        address,
        integration_id: parsed.integration_id,
        account_key: parsed.account_key,
        surface,
        token: parsed.token,
    })
}

/// Read bounded UTF-8 hook payload from `reader` (`<= MAX_RAW_HOOK_BYTES + 1`).
pub fn read_bounded_stdin<R: Read>(reader: R) -> Result<String, HelperError> {
    let mut buf = Vec::with_capacity(1024);
    reader
        .take((MAX_RAW_HOOK_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|_| HelperError::StdinIo)?;
    if buf.len() > MAX_RAW_HOOK_BYTES {
        return Err(HelperError::PayloadTooLarge);
    }
    String::from_utf8(buf).map_err(|_| HelperError::InvalidUtf8)
}

/// Generate one UUID `eventId` and UTC timestamps for this invocation, normalize via
/// [`adapters::codex::normalize_hook`], and serialize the metadata JSON before transport.
pub fn normalize_hook_payload(
    raw_hook: &str,
    config: &HelperConfig,
) -> Result<(NormalizedEvent, String), HelperError> {
    let event_id = Uuid::new_v4().to_string();
    let now_utc = format_canonical_utc(DateTime::<Utc>::from(SystemTime::now()));

    let context = CodexContext {
        integration_id: config.integration_id.clone(),
        account_key: config.account_key.clone(),
        surface: config.surface,
        event_id,
        observed_at: now_utc.clone(),
        received_at: now_utc,
    };

    let event = normalize_hook(raw_hook, &context).map_err(|_| HelperError::Normalization)?;
    let body = serde_json::to_string(&event).map_err(|_| HelperError::Normalization)?;
    Ok((event, body))
}

/// Deliver a pre-serialized [`NormalizedEvent`] body over loopback HTTP within `deadline`.
///
/// Performs at most 2 total attempts, retrying only on network errors or HTTP 429/503
/// using the exact same serialized metadata body and `eventId`.
pub fn deliver_serialized_event(
    config: &HelperConfig,
    body: &str,
    deadline: Instant,
) -> Result<DeliveryOutcome, HelperError> {
    if body.len() > MAX_RAW_HOOK_BYTES {
        return Err(HelperError::PayloadTooLarge);
    }
    match config.address {
        SocketAddr::V4(v4) if *v4.ip() == Ipv4Addr::LOCALHOST && v4.port() != 0 => {}
        _ => return Err(HelperError::InvalidLoopbackAddress),
    }

    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         Authorization: Bearer {token}\r\n\
         X-AUM-Integration: {integration}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        path = CODEX_EVENTS_PATH,
        host = config.address,
        token = config.token,
        integration = config.integration_id,
        len = body.len(),
    );

    let mut last_err = HelperError::Timeout;
    for attempt in 1..=MAX_ATTEMPTS {
        let attempt_deadline = deadline.min(Instant::now() + Duration::from_millis(85));
        match send_once(config.address, request.as_bytes(), attempt_deadline) {
            Ok(200) => return Ok(DeliveryOutcome::Delivered { attempts: attempt }),
            Ok(status @ (429 | 503)) => {
                last_err = HelperError::HttpStatus(status);
                if attempt == MAX_ATTEMPTS {
                    return Err(last_err);
                }
            }
            Ok(status) => return Err(HelperError::HttpStatus(status)),
            Err(err) => {
                last_err = err;
                if attempt == MAX_ATTEMPTS || Instant::now() >= deadline {
                    return Err(last_err);
                }
            }
        }
    }

    Err(last_err)
}

/// Read bounded stdin, normalize and serialize metadata before transport, and deliver to loopback.
pub fn process_hook<R: Read>(
    config: &HelperConfig,
    reader: R,
    deadline: Instant,
) -> Result<DeliveryOutcome, HelperError> {
    let raw_hook = read_bounded_stdin(reader)?;
    let (_event, body) = normalize_hook_payload(&raw_hook, config)?;
    deliver_serialized_event(config, &body, deadline)
}

/// Load config from `config_path`, read bounded stdin, normalize, and deliver within `deadline`.
pub fn run_worker<R: Read>(
    config_path: &Path,
    reader: R,
    deadline: Instant,
) -> Result<DeliveryOutcome, HelperError> {
    let config = load_config(config_path)?;
    process_hook(&config, reader, deadline)
}

fn remaining_budget(deadline: Instant) -> Result<Duration, HelperError> {
    let now = Instant::now();
    if now >= deadline {
        return Err(HelperError::Timeout);
    }
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        Err(HelperError::Timeout)
    } else {
        Ok(remaining)
    }
}

fn map_io_err(err: &std::io::Error, deadline: Instant) -> HelperError {
    if Instant::now() >= deadline
        || matches!(
            err.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        )
    {
        HelperError::Timeout
    } else {
        HelperError::Network
    }
}

fn send_once(addr: SocketAddr, request: &[u8], deadline: Instant) -> Result<u16, HelperError> {
    let connect_budget = remaining_budget(deadline)?;
    let mut stream =
        TcpStream::connect_timeout(&addr, connect_budget).map_err(|e| map_io_err(&e, deadline))?;

    let mut written = 0;
    while written < request.len() {
        let write_budget = remaining_budget(deadline)?;
        stream
            .set_write_timeout(Some(write_budget))
            .map_err(|_| HelperError::Network)?;
        match stream.write(&request[written..]) {
            Ok(0) => return Err(HelperError::Network),
            Ok(n) => written += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(map_io_err(&e, deadline)),
        }
    }

    let mut status_buf = [0u8; 256];
    let mut read_len = 0;
    while read_len < status_buf.len() {
        let read_budget = remaining_budget(deadline)?;
        stream
            .set_read_timeout(Some(read_budget))
            .map_err(|_| HelperError::Network)?;
        match stream.read(&mut status_buf[read_len..]) {
            Ok(0) => break,
            Ok(n) => {
                read_len += n;
                if status_buf[..read_len].contains(&b'\n') {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(map_io_err(&e, deadline)),
        }
    }

    parse_status_code(&status_buf[..read_len]).ok_or(HelperError::Network)
}

fn parse_status_code(buf: &[u8]) -> Option<u16> {
    let line_end = buf.iter().position(|&b| b == b'\n').unwrap_or(buf.len());
    let line = std::str::from_utf8(&buf[..line_end])
        .ok()?
        .trim_end_matches('\r');
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return None;
    }
    let code = parts.next()?;
    if code.len() != 3 {
        return None;
    }
    let code = code.parse::<u16>().ok()?;
    (100..=599).contains(&code).then_some(code)
}
