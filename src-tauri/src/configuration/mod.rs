//! User settings and Gemini credentials.
//!
//! Settings are stored as JSON in the platform config directory. The API key
//! is never written there: it comes from `GEMINI_API_KEY` (environment or a
//! `.env` file loaded by the backend) or from the OS credential store
//! (Secret Service on Linux, Credential Manager on Windows).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{ErrorKind, HaloError, HaloResult};
use crate::image_processing::{EncodingPreference, ProcessingOptions};

/// Default model: a stable Flash model from the current Gemini model list,
/// verified live with image input and streaming. (gemini-3.8-flash also
/// validates but returned "high demand" 503s during testing.) Users can change it; the backend validates it against
/// the live `models.get` endpoint from the settings screen.
pub const DEFAULT_MODEL: &str = "gemini-3.6-flash";
pub const DEFAULT_SHORTCUT: &str = "Ctrl+Shift+Space";
pub const MIN_DIMENSION: u32 = 256;
pub const MAX_DIMENSION: u32 = 4096;

const KEYRING_SERVICE: &str = "com.halo.app";
const KEYRING_USER: &str = "gemini-api-key";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub model: String,
    pub shortcut: String,
    pub max_dimension: u32,
    pub jpeg_quality: u8,
    pub encoding: EncodingPreference,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: std::env::var("GEMINI_MODEL")
                .ok()
                .filter(|m| is_valid_model_id(m))
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            shortcut: DEFAULT_SHORTCUT.to_string(),
            max_dimension: 2048,
            jpeg_quality: 90,
            encoding: EncodingPreference::Auto,
        }
    }
}

impl Settings {
    pub fn processing_options(&self) -> ProcessingOptions {
        ProcessingOptions {
            max_dimension: self.max_dimension,
            jpeg_quality: self.jpeg_quality,
            encoding: self.encoding,
        }
    }

    /// Validate user-supplied settings. The shortcut is validated
    /// separately by the shortcut parser.
    pub fn validate(&self) -> HaloResult<()> {
        let bad = |m: String| Err(HaloError::new(ErrorKind::Configuration, m));
        if !is_valid_model_id(&self.model) {
            return bad(format!(
                "Model id \"{}\" is not valid. Use an id such as {DEFAULT_MODEL}.",
                self.model
            ));
        }
        if !(MIN_DIMENSION..=MAX_DIMENSION).contains(&self.max_dimension) {
            return bad(format!(
                "Maximum image dimension must be between {MIN_DIMENSION} and {MAX_DIMENSION} px."
            ));
        }
        if !(50..=100).contains(&self.jpeg_quality) {
            return bad("JPEG quality must be between 50 and 100.".into());
        }
        Ok(())
    }
}

/// Model ids are interpolated into the request URL path, so only accept the
/// characters Gemini model ids actually use.
pub fn is_valid_model_id(model: &str) -> bool {
    let model = model.strip_prefix("models/").unwrap_or(model);
    !model.is_empty()
        && model.len() <= 100
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
        && !model.starts_with('.')
}

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// Load settings; a missing file yields defaults, a corrupt file is
/// reported (and defaults are used) rather than silently overwritten.
pub fn load_settings(config_dir: &Path) -> (Settings, Option<HaloError>) {
    let (mut settings, warning) = load_settings_file(config_dir);
    // GEMINI_MODEL, when set, wins over the saved model (like GEMINI_API_KEY).
    if let Some(model) = std::env::var("GEMINI_MODEL")
        .ok()
        .filter(|m| is_valid_model_id(m))
    {
        settings.model = model;
    }
    (settings, warning)
}

fn load_settings_file(config_dir: &Path) -> (Settings, Option<HaloError>) {
    let path = settings_path(config_dir);
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(s) if s.validate().is_ok() => (s, None),
            Ok(_) | Err(_) => (
                Settings::default(),
                Some(HaloError::new(
                    ErrorKind::Configuration,
                    format!(
                        "Ignored invalid settings file at {}; using defaults.",
                        path.display()
                    ),
                )),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(e) => (
            Settings::default(),
            Some(HaloError::new(
                ErrorKind::Configuration,
                format!("Could not read settings: {e}"),
            )),
        ),
    }
}

pub fn save_settings(config_dir: &Path, settings: &Settings) -> HaloResult<()> {
    settings.validate()?;
    fs::create_dir_all(config_dir).map_err(|e| {
        HaloError::new(
            ErrorKind::Configuration,
            format!("Could not create config dir: {e}"),
        )
    })?;
    let json =
        serde_json::to_string_pretty(settings).map_err(|e| HaloError::internal(e.to_string()))?;
    let path = settings_path(config_dir);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)
        .and_then(|_| fs::rename(&tmp, &path))
        .map_err(|e| {
            HaloError::new(
                ErrorKind::Configuration,
                format!("Could not save settings: {e}"),
            )
        })
}

// ---------------------------------------------------------------- API key

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySource {
    Environment,
    CredentialStore,
    None,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyStatus {
    pub source: KeySource,
    pub configured: bool,
    /// Whether the OS credential store is reachable (for saving a key).
    pub credential_store_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_store_error: Option<String>,
}

pub fn validate_api_key_format(key: &str) -> HaloResult<()> {
    let ok = (20..=200).contains(&key.len())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(HaloError::new(
            ErrorKind::Credentials,
            "That does not look like a Gemini API key (expected 20-200 letters, digits, '-', '_' or '.').",
        ))
    }
}

fn env_key() -> Option<String> {
    std::env::var("GEMINI_API_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

fn entry() -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
}

fn store_key() -> Result<Option<String>, String> {
    match entry().and_then(|e| e.get_password()) {
        Ok(k) => Ok(Some(k)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Resolve the API key. Environment wins so developers can override a
/// stored key without touching the credential store.
pub fn resolve_api_key() -> HaloResult<String> {
    if let Some(k) = env_key() {
        return Ok(k);
    }
    match store_key() {
        Ok(Some(k)) => Ok(k),
        Ok(None) => Err(missing_key_error()),
        Err(e) => Err(HaloError::new(
            ErrorKind::MissingApiKey,
            format!(
                "No GEMINI_API_KEY in the environment and the credential store could not be read ({e})."
            ),
        )),
    }
}

pub fn missing_key_error() -> HaloError {
    HaloError::new(
        ErrorKind::MissingApiKey,
        "No Gemini API key configured. Add one in Settings or set GEMINI_API_KEY.",
    )
}

pub fn key_status() -> KeyStatus {
    let store = store_key();
    let (available, err) = match &store {
        Ok(_) => (true, None),
        Err(e) => (false, Some(e.clone())),
    };
    let source = if env_key().is_some() {
        KeySource::Environment
    } else if matches!(store, Ok(Some(_))) {
        KeySource::CredentialStore
    } else {
        KeySource::None
    };
    KeyStatus {
        source,
        configured: source != KeySource::None,
        credential_store_available: available,
        credential_store_error: err,
    }
}

pub fn save_api_key(key: &str) -> HaloResult<()> {
    let key = key.trim();
    validate_api_key_format(key)?;
    entry()
        .and_then(|e| e.set_password(key))
        .map_err(|e| {
            HaloError::new(
                ErrorKind::Credentials,
                format!(
                    "Could not save the key to the OS credential store ({e}). \
                     Set GEMINI_API_KEY in the environment instead; Halo will not store keys in plaintext."
                ),
            )
        })
}

pub fn clear_api_key() -> HaloResult<()> {
    match entry().and_then(|e| e.delete_credential()) {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(HaloError::new(
            ErrorKind::Credentials,
            format!("Could not remove the key: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("halo-test-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn model_id_validation() {
        assert!(is_valid_model_id("gemini-3.8-flash"));
        assert!(is_valid_model_id("models/gemini-2.5-flash"));
        assert!(is_valid_model_id("gemini_custom.1"));
        assert!(!is_valid_model_id(""));
        assert!(!is_valid_model_id("../../v1/files"));
        assert!(!is_valid_model_id("gemini flash"));
        assert!(!is_valid_model_id("gemini?key=x"));
        assert!(!is_valid_model_id(&"a".repeat(101)));
    }

    #[test]
    fn settings_validation_bounds() {
        let mut s = Settings {
            model: DEFAULT_MODEL.into(),
            ..Settings::default()
        };
        assert!(s.validate().is_ok());
        s.max_dimension = 100;
        assert!(s.validate().is_err());
        s.max_dimension = 2048;
        s.jpeg_quality = 30;
        assert!(s.validate().is_err());
        s.jpeg_quality = 90;
        s.model = "bad model".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let dir = temp_dir("roundtrip");
        let s = Settings {
            model: "gemini-3.6-flash".into(),
            shortcut: "Ctrl+Alt+H".into(),
            max_dimension: 1024,
            jpeg_quality: 80,
            encoding: EncodingPreference::Jpeg,
        };
        save_settings(&dir, &s).unwrap();
        let (loaded, err) = load_settings(&dir);
        assert!(err.is_none());
        assert_eq!(loaded, s);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_settings_file_gives_defaults_without_error() {
        let dir = temp_dir("missing");
        let (s, err) = load_settings(&dir);
        assert!(err.is_none());
        assert_eq!(s.max_dimension, Settings::default().max_dimension);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn corrupt_settings_file_is_reported() {
        let dir = temp_dir("corrupt");
        fs::write(settings_path(&dir), "{ not json").unwrap();
        let (_, err) = load_settings(&dir);
        assert_eq!(err.unwrap().kind, ErrorKind::Configuration);
        fs::write(settings_path(&dir), r#"{"maxDimension": 5}"#).unwrap();
        let (s, err) = load_settings(&dir);
        assert!(err.is_some());
        assert_eq!(s.max_dimension, 2048);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn partial_settings_file_fills_defaults() {
        let dir = temp_dir("partial");
        fs::write(settings_path(&dir), r#"{"jpegQuality": 75}"#).unwrap();
        let (s, err) = load_settings(&dir);
        assert!(err.is_none());
        assert_eq!(s.jpeg_quality, 75);
        assert_eq!(s.shortcut, DEFAULT_SHORTCUT);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn save_rejects_invalid_settings() {
        let dir = temp_dir("invalid");
        let s = Settings {
            max_dimension: 1,
            ..Settings::default()
        };
        assert!(save_settings(&dir, &s).is_err());
        assert!(!settings_path(&dir).exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn api_key_format_validation() {
        assert!(validate_api_key_format("AIzaSyD-abcdefghijklmnopqrstuvwxyz012").is_ok());
        assert!(validate_api_key_format("short").is_err());
        assert!(validate_api_key_format("has spaces in it but is long enough").is_err());
        assert!(validate_api_key_format("AIza\nnewline-injection-attempt-xxxx").is_err());
    }
}
