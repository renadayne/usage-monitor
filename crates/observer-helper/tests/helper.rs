use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use normalized_schema::{EventType, ModelEvidenceKind, NormalizedEvent, Provider};
use observer_helper::{parse_config, process_hook, DeliveryOutcome, HelperError, MAX_CONFIG_BYTES};
use tempfile::NamedTempFile;

const VALID_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Debug)]
struct CapturedRequest {
    request_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk).expect("read request headers");
        assert!(n > 0, "unexpected EOF before HTTP headers completed");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let header_str = std::str::from_utf8(&buf[..header_end - 4]).expect("utf8 headers");
    let mut lines = header_str.split("\r\n");
    let request_line = lines.next().expect("request line").to_owned();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_owned();
            let val = v.trim().to_owned();
            if key.eq_ignore_ascii_case("content-length") {
                content_length = val.parse().expect("valid Content-Length");
            }
            headers.push((key, val));
        }
    }

    while buf.len() < header_end + content_length {
        let n = stream.read(&mut chunk).expect("read request body");
        assert!(n > 0, "unexpected EOF while reading HTTP body");
        buf.extend_from_slice(&chunk[..n]);
    }

    let body = String::from_utf8(buf[header_end..header_end + content_length].to_vec())
        .expect("utf8 body");
    CapturedRequest {
        request_line,
        headers,
        body,
    }
}

fn config_json(addr: &str, surface: &str, token: &str) -> String {
    format!(
        r#"{{
            "address": "{addr}",
            "integrationId": "codex-vscode-primary",
            "accountKey": "acct-local-1",
            "surface": "{surface}",
            "token": "{token}"
        }}"#
    )
}

#[test]
fn retries_503_then_200_with_identical_sanitized_metadata_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let addr = listener.local_addr().expect("local addr");

    let server_handle = thread::spawn(move || {
        let mut captured = Vec::with_capacity(2);

        let (mut s1, _) = listener.accept().expect("accept first attempt");
        captured.push(read_http_request(&mut s1));
        s1.write_all(
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .expect("write 503");
        let _ = s1.flush();

        let (mut s2, _) = listener.accept().expect("accept second attempt");
        captured.push(read_http_request(&mut s2));
        s2.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .expect("write 200");
        let _ = s2.flush();

        captured
    });

    let cfg = parse_config(&config_json(
        &addr.to_string(),
        "codex_vscode_extension",
        VALID_TOKEN,
    ))
    .expect("valid loopback config");

    let raw_hook = r#"{
        "hook_event_name": "UserPromptSubmit",
        "session_id": "sess-codex-99",
        "turn_id": "turn-42",
        "model": "gpt-5-codex",
        "prompt": "SECRET_PROMPT_MARKER_NEVER_SEND",
        "response": "SECRET_RESPONSE_MARKER_NEVER_SEND",
        "cwd": "C:\\Users\\alice\\private_repo",
        "transcript_path": "/home/alice/.codex/sessions/secret_transcript.jsonl"
    }"#;

    let deadline = Instant::now() + Duration::from_secs(2);
    let outcome = process_hook(&cfg, Cursor::new(raw_hook.as_bytes()), deadline)
        .expect("delivery should succeed on retry");
    assert_eq!(outcome, DeliveryOutcome::Delivered { attempts: 2 });

    let requests = server_handle.join().expect("server thread join");
    assert_eq!(requests.len(), 2);

    for req in &requests {
        assert_eq!(req.request_line, "POST /events/codex HTTP/1.1");
        assert_eq!(req.header("Host"), Some(addr.to_string().as_str()));
        assert_eq!(req.header("Content-Type"), Some("application/json"));
        assert_eq!(
            req.header("Authorization"),
            Some(format!("Bearer {VALID_TOKEN}").as_str())
        );
        assert_eq!(
            req.header("X-AUM-Integration"),
            Some("codex-vscode-primary")
        );
        assert!(!req.body.contains("SECRET_PROMPT_MARKER_NEVER_SEND"));
        assert!(!req.body.contains("SECRET_RESPONSE_MARKER_NEVER_SEND"));
        assert!(!req.body.contains("private_repo"));
        assert!(!req.body.contains("secret_transcript"));
    }

    assert_eq!(
        requests[0].body, requests[1].body,
        "retry must send the exact same serialized metadata body and eventId"
    );

    let event: NormalizedEvent =
        serde_json::from_str(&requests[0].body).expect("valid NormalizedEvent JSON");
    event.validate().expect("normalized event validates");
    assert_eq!(event.provider, Provider::Codex);
    assert_eq!(event.event_type, EventType::ModelObserved);
    assert_eq!(event.integration_id, "codex-vscode-primary");
    assert_eq!(event.account_key, "acct-local-1");
    assert_eq!(event.surface, "codex_vscode_extension");
    assert_eq!(event.source_session_id, "sess-codex-99");
    assert_eq!(event.source_turn_id.as_deref(), Some("turn-42"));
    assert_eq!(event.model_selected.as_deref(), Some("gpt-5-codex"));
    assert_eq!(
        event.model_evidence_kind,
        ModelEvidenceKind::HookCommonMetadata
    );
    assert_eq!(event.evidence_ref.as_deref(), Some(event.event_id.as_str()));
}

#[test]
fn rejects_invalid_remote_config_and_invalid_token() {
    // Non-loopback IPv4 address rejected
    assert_eq!(
        parse_config(&config_json(
            "192.168.1.25:9100",
            "codex_vscode_extension",
            VALID_TOKEN
        ))
        .err(),
        Some(HelperError::InvalidLoopbackAddress)
    );

    // 127.0.0.2 (non-127.0.0.1) rejected
    assert_eq!(
        parse_config(&config_json(
            "127.0.0.2:9100",
            "codex_vscode_extension",
            VALID_TOKEN
        ))
        .err(),
        Some(HelperError::InvalidLoopbackAddress)
    );

    // IPv6 loopback rejected (require 127.0.0.1:PORT)
    assert_eq!(
        parse_config(&config_json(
            "[::1]:9100",
            "codex_vscode_extension",
            VALID_TOKEN
        ))
        .err(),
        Some(HelperError::InvalidLoopbackAddress)
    );

    // Port 0 rejected
    assert_eq!(
        parse_config(&config_json(
            "127.0.0.1:0",
            "codex_vscode_extension",
            VALID_TOKEN
        ))
        .err(),
        Some(HelperError::InvalidLoopbackAddress)
    );

    // Invalid surface rejected
    assert_eq!(
        parse_config(&config_json(
            "127.0.0.1:9100",
            "unknown_surface",
            VALID_TOKEN
        ))
        .err(),
        Some(HelperError::InvalidSurface)
    );

    // Short token rejected
    assert_eq!(
        parse_config(&config_json(
            "127.0.0.1:9100",
            "codex_desktop_app",
            "deadbeef"
        ))
        .err(),
        Some(HelperError::InvalidToken)
    );

    // Non-hex 64-char token rejected
    let non_hex_token = "g123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert_eq!(
        parse_config(&config_json(
            "127.0.0.1:9100",
            "codex_desktop_app",
            non_hex_token
        ))
        .err(),
        Some(HelperError::InvalidToken)
    );

    // Unknown field rejected
    let unknown_field_cfg = format!(
        r#"{{
            "address": "127.0.0.1:9100",
            "integrationId": "codex-desktop",
            "accountKey": "acct-1",
            "surface": "codex_desktop_app",
            "token": "{VALID_TOKEN}",
            "unexpected": true
        }}"#
    );
    assert_eq!(
        parse_config(&unknown_field_cfg).err(),
        Some(HelperError::InvalidConfigJson)
    );

    // Oversized config (>16KiB) rejected
    let oversized = " ".repeat(MAX_CONFIG_BYTES + 1);
    assert_eq!(
        parse_config(&oversized).err(),
        Some(HelperError::ConfigTooLarge)
    );
}

#[test]
fn binary_with_open_stdin_exits_zero_with_empty_json_within_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");

    let mut cfg_file = NamedTempFile::new().expect("create temp config");
    cfg_file
        .write_all(config_json(&addr.to_string(), "codex_desktop_app", VALID_TOKEN).as_bytes())
        .expect("write temp config");
    cfg_file.flush().expect("flush temp config");

    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_aum-observer"))
        .arg("--config")
        .arg(cfg_file.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aum-observer binary");

    // Keep stdin open without writing EOF so the worker thread blocks on stdin read.
    let _open_stdin = child.stdin.take().expect("take child stdin");

    let max_wait = Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait child") {
            break status;
        }
        assert!(
            start.elapsed() < max_wait,
            "aum-observer did not exit within 2s cap while stdin was held open"
        );
        thread::sleep(Duration::from_millis(10));
    };

    assert!(status.success(), "aum-observer must exit with code 0");

    let mut stdout_str = String::new();
    child
        .stdout
        .take()
        .expect("child stdout")
        .read_to_string(&mut stdout_str)
        .expect("read stdout");
    assert_eq!(stdout_str, "{}");

    let mut stderr_str = String::new();
    child
        .stderr
        .take()
        .expect("child stderr")
        .read_to_string(&mut stderr_str)
        .expect("read stderr");
    assert_eq!(stderr_str, "");
}
