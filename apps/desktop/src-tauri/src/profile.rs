//! App-owned observer helper profile initialization and validation.

use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use adapters::codex::Surface;
use observer_helper::{parse_config, HelperError, MAX_CONFIG_BYTES};
use observer_runtime::ObserverBinding;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

pub const VSCODE_PROFILE_FILENAME: &str = "codex-vscode.json";
pub const DESKTOP_PROFILE_FILENAME: &str = "codex-desktop.json";
pub const VSCODE_INTEGRATION_ID: &str = "codex-vscode-local";
pub const DESKTOP_INTEGRATION_ID: &str = "codex-desktop-local";
pub const VSCODE_DEFAULT_ACCOUNT_KEY: &str = "local-unverified-vscode";
pub const DESKTOP_DEFAULT_ACCOUNT_KEY: &str = "local-unverified-desktop";

/// Validated app-owned observer helper profile without `Debug`, `Display`, or serde traits.
pub struct Profile {
    config_path: PathBuf,
    integration_id: String,
    account_key: String,
    surface: Surface,
    token: String,
}

impl Profile {
    pub fn binding(&self) -> Result<ObserverBinding, &'static str> {
        ObserverBinding::new(
            self.integration_id.clone(),
            self.account_key.clone(),
            self.surface,
            self.token.clone(),
        )
    }

    #[must_use]
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    #[must_use]
    pub fn integration_id(&self) -> &str {
        &self.integration_id
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileConfigDto {
    address: String,
    integration_id: String,
    account_key: String,
    surface: String,
    token: String,
}

const fn surface_defaults(surface: Surface) -> (&'static str, &'static str, &'static str) {
    match surface {
        Surface::VsCodeExtension => (
            VSCODE_PROFILE_FILENAME,
            VSCODE_INTEGRATION_ID,
            VSCODE_DEFAULT_ACCOUNT_KEY,
        ),
        Surface::StandaloneDesktop => (
            DESKTOP_PROFILE_FILENAME,
            DESKTOP_INTEGRATION_ID,
            DESKTOP_DEFAULT_ACCOUNT_KEY,
        ),
    }
}

fn validate_loopback_address(address: SocketAddr) -> Result<(), &'static str> {
    match address {
        SocketAddr::V4(v4) if *v4.ip() == Ipv4Addr::LOCALHOST && v4.port() != 0 => Ok(()),
        _ => Err("observer address must be 127.0.0.1 with a non-zero port"),
    }
}

/// Ensure an app-owned observer helper profile exists and matches the surface contract.
pub fn ensure_profile(
    directory: &Path,
    surface: Surface,
    address: SocketAddr,
) -> Result<Profile, &'static str> {
    validate_loopback_address(address)?;
    if directory.as_os_str().is_empty() {
        return Err("profile directory must not be empty");
    }
    fs::create_dir_all(directory).map_err(|_| "cannot create profile directory")?;
    let dir_meta =
        fs::symlink_metadata(directory).map_err(|_| "cannot inspect profile directory")?;
    if dir_meta.file_type().is_symlink() || !dir_meta.file_type().is_dir() {
        return Err("profile directory must be a real directory");
    }

    let (filename, expected_id, default_account_key) = surface_defaults(surface);
    let config_path = directory.join(filename);

    match fs::symlink_metadata(&config_path) {
        Ok(_) => return read_and_validate(config_path, surface, expected_id, address),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect profile file"),
    }

    let dto = ProfileConfigDto {
        address: address.to_string(),
        integration_id: expected_id.to_owned(),
        account_key: default_account_key.to_owned(),
        surface: surface.as_str().to_owned(),
        token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
    };
    let encoded = serde_json::to_vec_pretty(&dto).map_err(|_| "cannot serialize profile config")?;

    let mut temp =
        NamedTempFile::new_in(directory).map_err(|_| "cannot create temporary profile file")?;
    temp.as_file_mut()
        .write_all(&encoded)
        .map_err(|_| "cannot write temporary profile file")?;
    temp.as_file_mut()
        .sync_all()
        .map_err(|_| "cannot sync temporary profile file")?;

    match temp.persist_noclobber(&config_path) {
        Ok(_) => read_and_validate(config_path, surface, expected_id, address),
        Err(err) if err.error.kind() == ErrorKind::AlreadyExists => {
            read_and_validate(config_path, surface, expected_id, address)
        }
        Err(_) => Err("cannot persist profile file"),
    }
}

fn read_and_validate(
    config_path: PathBuf,
    expected_surface: Surface,
    expected_id: &str,
    expected_address: SocketAddr,
) -> Result<Profile, &'static str> {
    let meta = fs::symlink_metadata(&config_path).map_err(|_| "cannot inspect profile file")?;
    if meta.file_type().is_symlink() {
        return Err("profile file must not be a symlink");
    }
    if !meta.file_type().is_file() {
        return Err("profile path must be a regular file");
    }

    let file = File::open(&config_path).map_err(|_| "cannot open profile file")?;
    let mut buf = Vec::with_capacity(512);
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|_| "cannot read profile file")?;
    if buf.len() > MAX_CONFIG_BYTES {
        return Err("profile file exceeds 16 KiB limit");
    }

    let raw = std::str::from_utf8(&buf).map_err(|_| "profile file is not valid UTF-8")?;
    let helper_cfg = parse_config(raw).map_err(map_helper_error)?;
    let dto: ProfileConfigDto =
        serde_json::from_str(raw).map_err(|_| "invalid profile config JSON")?;

    if helper_cfg.integration_id() != expected_id || dto.integration_id != expected_id {
        return Err("profile integrationId does not match surface");
    }
    if helper_cfg.surface() != expected_surface || dto.surface != expected_surface.as_str() {
        return Err("profile surface does not match expected surface");
    }
    if helper_cfg.address() != expected_address {
        return Err("profile address does not match expected loopback address");
    }

    let profile = Profile {
        config_path,
        integration_id: helper_cfg.integration_id().to_owned(),
        account_key: helper_cfg.account_key().to_owned(),
        surface: helper_cfg.surface(),
        token: dto.token,
    };
    let _ = profile.binding()?;
    Ok(profile)
}

const fn map_helper_error(err: HelperError) -> &'static str {
    match err {
        HelperError::ConfigTooLarge => "profile file exceeds 16 KiB limit",
        HelperError::InvalidConfigJson => "invalid profile config JSON",
        HelperError::InvalidLoopbackAddress => "invalid profile loopback address",
        HelperError::InvalidIdentifier => "invalid profile identifier",
        HelperError::InvalidSurface => "invalid profile surface",
        HelperError::InvalidToken => "invalid profile token",
        _ => "invalid profile configuration",
    }
}
