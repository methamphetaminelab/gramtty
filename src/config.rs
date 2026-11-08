use std::path::PathBuf;

use serde::Deserialize;

pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

pub struct ApiCredentials {
    pub api_id: i32,
    pub api_hash: String,
}

impl std::fmt::Debug for ApiCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiCredentials").finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct ApiFile {
    api_id: i32,
    api_hash: String,
}

pub fn env_prefix() -> String {
    APP_NAME.to_uppercase().replace('-', "_")
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())
}

fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf, String> {
    match std::env::var_os(var).filter(|v| !v.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(home()?.join(fallback)),
    }
}

pub fn config_dir() -> Result<PathBuf, String> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join(APP_NAME))
}

pub fn data_dir() -> Result<PathBuf, String> {
    if let Some(dir) =
        std::env::var_os(format!("{}_DATA_DIR", env_prefix())).filter(|v| !v.is_empty())
    {
        return Ok(PathBuf::from(dir));
    }
    Ok(xdg_dir("XDG_DATA_HOME", ".local/share")?.join(APP_NAME))
}

pub fn load_credentials() -> Result<ApiCredentials, String> {
    let prefix = env_prefix();
    let id_var = format!("{prefix}_API_ID");
    let hash_var = format!("{prefix}_API_HASH");
    if let (Ok(id), Ok(hash)) = (std::env::var(&id_var), std::env::var(&hash_var)) {
        let api_id = id
            .trim()
            .parse()
            .map_err(|_| format!("{id_var} is not a number"))?;
        return Ok(ApiCredentials {
            api_id,
            api_hash: hash.trim().to_owned(),
        });
    }
    let path = config_dir()?.join("api.toml");
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "cannot read {}: {e}; create it with api_id and api_hash or set {id_var}/{hash_var}",
            path.display()
        )
    })?;
    let file: ApiFile = toml::from_str(&raw)
        .map_err(|e| format!("{} is invalid: {}", path.display(), e.message()))?;
    if file.api_hash.trim().is_empty() {
        return Err(format!("{}: api_hash is empty", path.display()));
    }
    Ok(ApiCredentials {
        api_id: file.api_id,
        api_hash: file.api_hash.trim().to_owned(),
    })
}

pub fn keymap_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("keymap.toml"))
}
