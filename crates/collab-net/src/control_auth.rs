//! Native authentication for the local desktop control channel.
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;

use figcraft_collab::same_secret;

/// A validated control-channel token. Its contents are intentionally absent from Debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct ControlToken(String);

impl fmt::Debug for ControlToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ControlToken([REDACTED])")
    }
}

impl ControlToken {
    /// Read and validate the process-scoped control token.
    pub fn from_env() -> io::Result<Self> {
        let value = std::env::var_os("FIGCRAFT_CONTROL_TOKEN");
        Self::from_os_value(value)
    }

    /// The token the app uses for `--control <port>`: `FIGCRAFT_CONTROL_TOKEN` when set, else a fresh
    /// random one written to the per-user file [`token_path`] so `figcraft-cli mcp --attach` finds it.
    pub fn for_server(port: u16) -> io::Result<Self> {
        if std::env::var_os("FIGCRAFT_CONTROL_TOKEN").is_some() {
            return Self::from_env();
        }
        let path = token_path(port).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no per-user directory for the control token; set FIGCRAFT_CONTROL_TOKEN"))?;
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw).map_err(|e| io::Error::other(format!("no secure random numbers: {e}")))?;
        let token = Self::new(&base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, raw))?;
        write_private(&path, token.as_str())?;
        Ok(token)
    }

    /// The token a client uses for the app on `port`: `FIGCRAFT_CONTROL_TOKEN` when set, else the
    /// app's per-user file.
    pub fn for_client(port: u16) -> io::Result<Self> {
        if std::env::var_os("FIGCRAFT_CONTROL_TOKEN").is_some() {
            return Self::from_env();
        }
        let path = token_path(port).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no per-user directory for the control token; set FIGCRAFT_CONTROL_TOKEN"))?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| io::Error::new(e.kind(), format!("no control token: FIGCRAFT_CONTROL_TOKEN is unset and {} is unreadable: {e}", path.display())))?;
        Self::new(text.trim())
    }

    /// Validate an explicit Unicode token.
    pub fn new(value: &str) -> io::Result<Self> {
        validate(value)?;
        Ok(Self(value.to_owned()))
    }

    fn from_os_value(value: Option<OsString>) -> io::Result<Self> {
        let value = value
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "FIGCRAFT_CONTROL_TOKEN is required when the control listener is enabled"))?;
        let value = value.into_string().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "FIGCRAFT_CONTROL_TOKEN must be Unicode"))?;
        Self::new(&value)
    }

    /// Compare a presented token with this token using the shared secret comparison helper.
    pub fn matches(&self, presented: &str) -> bool {
        same_secret(&self.0, presented)
    }

    /// Return the value for an authenticated protocol request.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The per-user file holding the token of the app listening on `port`.
pub fn token_path(port: u16) -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
    }?;
    base.is_absolute().then(|| base.join("figcraft").join(format!("control-token-{port}")))
}

/// Write `text` to `path` readable only by the current user (mode 0600 on Unix; per-user
/// directory ACLs on Windows).
fn write_private(path: &std::path::Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(text.as_bytes())
}

fn validate(value: &str) -> io::Result<()> {
    if !(22..=128).contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "FIGCRAFT_CONTROL_TOKEN must be 22 to 128 URL-safe ASCII characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn rejects_missing_empty_and_non_unicode_environment_values() {
        assert!(ControlToken::from_os_value(None).is_err());
        assert!(ControlToken::from_os_value(Some(OsString::new())).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert!(ControlToken::from_os_value(Some(OsString::from_vec(vec![0xff]))).is_err());
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            assert!(ControlToken::from_os_value(Some(OsString::from_wide(&[0xd800]))).is_err());
        }
    }

    #[test]
    fn a_token_written_for_a_client_is_read_back_and_the_path_names_the_port() {
        let dir = std::env::temp_dir().join(format!("figcraft-token-{}", std::process::id()));
        let path = dir.join("figcraft").join("control-token-1");
        write_private(&path, "a_secure_token_with_32_chars").unwrap();
        let read = ControlToken::new(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        assert!(read.matches("a_secure_token_with_32_chars"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(token_path(7979).is_none_or(|p| p.ends_with("control-token-7979")));
    }

    #[test]
    fn accepts_valid_tokens_and_rejects_invalid_tokens() {
        assert!(ControlToken::new("a".repeat(22).as_str()).is_ok());
        assert!(ControlToken::new("a".repeat(128).as_str()).is_ok());
        let invalid = [String::new(), "a".repeat(21), "a".repeat(129), "contains space 1234567890".into(), "unicode-é-123456789012".into()];
        for value in invalid {
            assert!(ControlToken::new(&value).is_err());
        }
        let token = ControlToken::new("a_secure_token_with_32_chars").unwrap();
        assert!(token.matches("a_secure_token_with_32_chars"));
        assert!(!token.matches("another_secure_token_with_32"));
        assert!(!format!("{token:?}").contains("secure"));
    }
}
