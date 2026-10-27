use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use adapters::codex::{Surface, SURFACE_STANDALONE_DESKTOP, SURFACE_VSCODE_EXTENSION};
use observer_helper::{load_config, MAX_CONFIG_BYTES};
use serde_json::Value;
use tempfile::TempDir;
use usage_monitor_desktop::profile::{
    ensure_profile, DESKTOP_DEFAULT_ACCOUNT_KEY, DESKTOP_INTEGRATION_ID, DESKTOP_PROFILE_FILENAME,
    VSCODE_DEFAULT_ACCOUNT_KEY, VSCODE_INTEGRATION_ID, VSCODE_PROFILE_FILENAME,
};

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[test]
fn restart_preserves_exact_bytes_and_token_and_respects_user_account_key() {
    let temp = TempDir::new().expect("tempdir");
    let profile_dir = temp.path().join("nested").join("profiles");
    let addr = loopback(4319);

    let first = ensure_profile(&profile_dir, Surface::VsCodeExtension, addr)
        .expect("initial profile creation should succeed");
    assert_eq!(first.integration_id(), VSCODE_INTEGRATION_ID);
    assert_eq!(
        first.config_path(),
        profile_dir.join(VSCODE_PROFILE_FILENAME).as_path()
    );

    let first_bytes = fs::read(first.config_path()).expect("read created profile");
    let parsed_json: Value = serde_json::from_slice(&first_bytes).expect("valid JSON");
    let obj = parsed_json.as_object().expect("JSON object");
    assert_eq!(obj.len(), 5);
    assert_eq!(obj["address"], "127.0.0.1:4319");
    assert_eq!(obj["integrationId"], VSCODE_INTEGRATION_ID);
    assert_eq!(obj["accountKey"], VSCODE_DEFAULT_ACCOUNT_KEY);
    assert_eq!(obj["surface"], SURFACE_VSCODE_EXTENSION);

    let token = obj["token"].as_str().expect("token string").to_owned();
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));

    // Helper must accept the generated file directly.
    let helper_cfg = load_config(first.config_path()).expect("observer-helper load_config");
    assert_eq!(helper_cfg.address(), addr);
    assert_eq!(helper_cfg.integration_id(), VSCODE_INTEGRATION_ID);
    assert_eq!(helper_cfg.account_key(), VSCODE_DEFAULT_ACCOUNT_KEY);
    assert_eq!(helper_cfg.surface(), Surface::VsCodeExtension);

    // Second invocation (restart) must preserve exact bytes and token.
    let second = ensure_profile(&profile_dir, Surface::VsCodeExtension, addr)
        .expect("restart profile load should succeed");
    let second_bytes = fs::read(second.config_path()).expect("read second profile");
    assert_eq!(first_bytes, second_bytes);

    // User-edited valid opaque accountKey must be preserved without overwriting.
    let custom_json = serde_json::json!({
        "address": "127.0.0.1:4319",
        "integrationId": VSCODE_INTEGRATION_ID,
        "accountKey": "acct-user-custom-01",
        "surface": SURFACE_VSCODE_EXTENSION,
        "token": token,
    });
    let custom_bytes = serde_json::to_vec_pretty(&custom_json).expect("serialize custom");
    fs::write(second.config_path(), &custom_bytes).expect("write custom accountKey");

    let third = ensure_profile(&profile_dir, Surface::VsCodeExtension, addr)
        .expect("custom accountKey profile should validate");
    assert_eq!(
        fs::read(third.config_path()).expect("read third"),
        custom_bytes
    );
    let binding = third.binding().expect("binding from custom accountKey");
    assert_eq!(binding.account_key, "acct-user-custom-01");
}

#[test]
fn separate_surfaces_have_distinct_ids_filenames_account_keys_and_tokens() {
    let temp = TempDir::new().expect("tempdir");
    let addr = loopback(4320);

    let vscode =
        ensure_profile(temp.path(), Surface::VsCodeExtension, addr).expect("vscode profile");
    let desktop =
        ensure_profile(temp.path(), Surface::StandaloneDesktop, addr).expect("desktop profile");

    assert_eq!(
        vscode.config_path(),
        temp.path().join(VSCODE_PROFILE_FILENAME).as_path()
    );
    assert_eq!(
        desktop.config_path(),
        temp.path().join(DESKTOP_PROFILE_FILENAME).as_path()
    );
    assert_eq!(vscode.integration_id(), VSCODE_INTEGRATION_ID);
    assert_eq!(desktop.integration_id(), DESKTOP_INTEGRATION_ID);

    let vscode_binding = vscode.binding().expect("vscode binding");
    let desktop_binding = desktop.binding().expect("desktop binding");
    assert_eq!(vscode_binding.integration_id, VSCODE_INTEGRATION_ID);
    assert_eq!(vscode_binding.account_key, VSCODE_DEFAULT_ACCOUNT_KEY);
    assert_eq!(vscode_binding.surface, Surface::VsCodeExtension);

    assert_eq!(desktop_binding.integration_id, DESKTOP_INTEGRATION_ID);
    assert_eq!(desktop_binding.account_key, DESKTOP_DEFAULT_ACCOUNT_KEY);
    assert_eq!(desktop_binding.surface, Surface::StandaloneDesktop);

    let vscode_json: Value =
        serde_json::from_slice(&fs::read(vscode.config_path()).unwrap()).unwrap();
    let desktop_json: Value =
        serde_json::from_slice(&fs::read(desktop.config_path()).unwrap()).unwrap();
    assert_eq!(vscode_json["surface"], SURFACE_VSCODE_EXTENSION);
    assert_eq!(desktop_json["surface"], SURFACE_STANDALONE_DESKTOP);
    assert_ne!(vscode_json["token"], desktop_json["token"]);
}

#[test]
fn tampered_malformed_and_oversized_profiles_are_retained_and_errors_sanitized() {
    let temp = TempDir::new().expect("tempdir");
    let addr = loopback(4321);
    let path = temp.path().join(VSCODE_PROFILE_FILENAME);
    let secret_token = "deadbeef".repeat(8);

    // 1. Malformed JSON with sensitive token text must not be overwritten or leaked in error.
    let malformed = format!("{{\"token\":\"{secret_token}\", broken json").into_bytes();
    fs::write(&path, &malformed).expect("write malformed");
    let err = ensure_profile(temp.path(), Surface::VsCodeExtension, addr)
        .err()
        .expect("malformed profile must fail");
    assert!(!err.contains(&secret_token));
    assert!(!err.contains("broken"));
    assert_eq!(fs::read(&path).expect("retained malformed"), malformed);

    // 2. Tampered integrationId, surface, address, or token must fail and preserve user bytes.
    let wrong_id = serde_json::to_vec(&serde_json::json!({
        "address": "127.0.0.1:4321",
        "integrationId": DESKTOP_INTEGRATION_ID,
        "accountKey": VSCODE_DEFAULT_ACCOUNT_KEY,
        "surface": SURFACE_VSCODE_EXTENSION,
        "token": secret_token,
    }))
    .unwrap();
    fs::write(&path, &wrong_id).unwrap();
    let err = ensure_profile(temp.path(), Surface::VsCodeExtension, addr)
        .err()
        .expect("mismatched integrationId must fail");
    assert!(!err.contains(&secret_token));
    assert_eq!(fs::read(&path).unwrap(), wrong_id);

    let wrong_addr = serde_json::to_vec(&serde_json::json!({
        "address": "127.0.0.1:9999",
        "integrationId": VSCODE_INTEGRATION_ID,
        "accountKey": VSCODE_DEFAULT_ACCOUNT_KEY,
        "surface": SURFACE_VSCODE_EXTENSION,
        "token": secret_token,
    }))
    .unwrap();
    fs::write(&path, &wrong_addr).unwrap();
    let err = ensure_profile(temp.path(), Surface::VsCodeExtension, addr)
        .err()
        .expect("mismatched address must fail");
    assert!(!err.contains(&secret_token));
    assert_eq!(fs::read(&path).unwrap(), wrong_addr);

    // 3. Snake-case keys must be rejected because config JSON requires exact camelCase fields.
    let snake_case = serde_json::to_vec(&serde_json::json!({
        "address": "127.0.0.1:4321",
        "integration_id": VSCODE_INTEGRATION_ID,
        "account_key": VSCODE_DEFAULT_ACCOUNT_KEY,
        "surface": SURFACE_VSCODE_EXTENSION,
        "token": secret_token,
    }))
    .unwrap();
    fs::write(&path, &snake_case).unwrap();
    assert!(ensure_profile(temp.path(), Surface::VsCodeExtension, addr).is_err());
    assert_eq!(fs::read(&path).unwrap(), snake_case);

    // 4. Oversized file (> 16 KiB) must be rejected and retained.
    let oversized = vec![b' '; MAX_CONFIG_BYTES + 128];
    fs::write(&path, &oversized).unwrap();
    let err = ensure_profile(temp.path(), Surface::VsCodeExtension, addr)
        .err()
        .expect("oversized profile must fail");
    assert!(err.contains("16 KiB"));
    assert_eq!(fs::read(&path).unwrap().len(), oversized.len());
}

#[test]
fn non_loopback_and_zero_port_addresses_are_rejected() {
    let temp = TempDir::new().expect("tempdir");
    let invalid_addrs = [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 4319),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 4319),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 4319),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 4319),
    ];

    for addr in invalid_addrs {
        let res = ensure_profile(temp.path(), Surface::VsCodeExtension, addr);
        assert!(res.is_err(), "expected {addr} to be rejected");
    }
    assert!(!temp.path().join(VSCODE_PROFILE_FILENAME).exists());
}

#[test]
fn binding_is_constructed_without_exposing_token_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let addr = loopback(4322);
    let profile = ensure_profile(temp.path(), Surface::VsCodeExtension, addr).expect("profile");

    let raw: Value =
        serde_json::from_slice(&fs::read(profile.config_path()).unwrap()).expect("json");
    let token = raw["token"].as_str().expect("token");

    assert!(!profile.integration_id().contains(token));
    assert!(!profile.config_path().to_string_lossy().contains(token));

    let binding = profile.binding().expect("binding");
    assert_eq!(binding.integration_id, VSCODE_INTEGRATION_ID);
    assert_eq!(binding.account_key, VSCODE_DEFAULT_ACCOUNT_KEY);
    assert_eq!(binding.surface, Surface::VsCodeExtension);
}
