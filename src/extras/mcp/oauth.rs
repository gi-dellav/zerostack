//! OAuth 2.0 (authorization code + PKCE) support for URL-based MCP servers.
//!
//! Tokens are persisted per server under `<data_dir>/mcp-oauth/<server>.json`
//! (mode 0600 on unix) via a file-backed [`CredentialStore`]. The interactive
//! login is driven from the `/mcp login <server>` slash command; afterwards the
//! stored refresh token lets startup reconnect without a browser.

use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationSession, CredentialStore,
    StoredCredentials,
};

use super::config::OAuthSettings;

const CLIENT_NAME: &str = "zerostack";

fn oauth_dir() -> PathBuf {
    crate::session::storage::data_dir().join("mcp-oauth")
}

/// Sanitize a server name into a single safe filename component.
pub(crate) fn token_filename(server_name: &str) -> String {
    let sanitized: String = server_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{sanitized}.json")
}

fn token_path(server_name: &str) -> PathBuf {
    oauth_dir().join(token_filename(server_name))
}

/// File-backed credential store. One JSON file per MCP server.
struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    fn new(server_name: &str) -> Self {
        Self {
            path: token_path(server_name),
        }
    }

    fn read_blocking(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AuthError::InternalError(format!("read token file: {e}"))),
        };
        let creds = serde_json::from_slice(&bytes)
            .map_err(|e| AuthError::InternalError(format!("parse token file: {e}")))?;
        Ok(Some(creds))
    }

    fn write_blocking(&self, creds: &StoredCredentials) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AuthError::InternalError(format!("create token dir: {e}")))?;
        }
        let bytes = serde_json::to_vec_pretty(creds)
            .map_err(|e| AuthError::InternalError(format!("serialize token: {e}")))?;
        let tmp = self.path.with_extension("json.tmp");
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| AuthError::InternalError(format!("create token file: {e}")))?;
        set_owner_only(&f);
        f.write_all(&bytes)
            .map_err(|e| AuthError::InternalError(format!("write token file: {e}")))?;
        f.sync_all().ok();
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| AuthError::InternalError(format!("persist token file: {e}")))?;
        Ok(())
    }

    fn clear_blocking(&self) -> Result<(), AuthError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AuthError::InternalError(format!("remove token file: {e}"))),
        }
    }
}

#[cfg(unix)]
fn set_owner_only(f: &std::fs::File) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = f.set_permissions(std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!("mcp oauth: failed to set 0600 permissions on token file: {e}");
    }
}

#[cfg(not(unix))]
fn set_owner_only(_f: &std::fs::File) {}

// The `CredentialStore` trait is declared with `#[async_trait]`, so its methods
// desugar to `-> Pin<Box<dyn Future + Send>>`. We implement that shape directly
// to avoid pulling in the `async-trait` proc-macro as a dependency.
type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AuthError>> + Send + 'a>>;

impl CredentialStore for FileCredentialStore {
    fn load<'life0, 'async_trait>(
        &'life0 self,
    ) -> StoreFuture<'async_trait, Option<StoredCredentials>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.read_blocking() })
    }

    fn save<'life0, 'async_trait>(
        &'life0 self,
        credentials: StoredCredentials,
    ) -> StoreFuture<'async_trait, ()>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.write_blocking(&credentials) })
    }

    fn clear<'life0, 'async_trait>(&'life0 self) -> StoreFuture<'async_trait, ()>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.clear_blocking() })
    }
}

/// Delete the stored token for a server. Returns whether a file was removed.
pub fn logout(server_name: &str) -> anyhow::Result<bool> {
    let path = token_path(server_name);
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

/// Build an [`AuthClient`] for a server that already has stored credentials.
///
/// Returns an error (without prompting) when no usable token is stored, so the
/// caller can tell the user to run `/mcp login`.
pub async fn build_auth_client(
    server_name: &str,
    url: &str,
    _settings: &OAuthSettings,
) -> anyhow::Result<AuthClient<reqwest::Client>> {
    let mut manager = AuthorizationManager::new(url)
        .await
        .map_err(|e| anyhow::anyhow!("OAuth init failed: {e}"))?;
    manager.set_credential_store(FileCredentialStore::new(server_name));

    let restored = manager
        .initialize_from_store()
        .await
        .map_err(|e| anyhow::anyhow!("OAuth restore failed: {e}"))?;
    if !restored {
        anyhow::bail!("no OAuth token stored; run `/mcp login {server_name}`");
    }

    Ok(AuthClient::new(reqwest::Client::new(), manager))
}

/// Result of starting an interactive login: the URL to open and the live session.
pub struct LoginSession {
    pub auth_url: String,
    session: AuthorizationSession,
    redirect_port: u16,
}

/// Begin an interactive OAuth login: discover metadata, register/authorize, and
/// return the URL the user must open. Call [`LoginSession::wait_for_callback`]
/// to complete the flow.
pub async fn begin_login(
    server_name: &str,
    url: &str,
    settings: &OAuthSettings,
) -> anyhow::Result<LoginSession> {
    let mut manager = AuthorizationManager::new(url)
        .await
        .map_err(|e| anyhow::anyhow!("OAuth init failed: {e}"))?;
    manager.set_credential_store(FileCredentialStore::new(server_name));

    let metadata = manager
        .discover_metadata()
        .await
        .map_err(|e| anyhow::anyhow!("OAuth metadata discovery failed: {e}"))?;
    manager.set_metadata(metadata);

    let redirect_uri = settings.redirect_uri();
    let scope_refs: Vec<&str> = settings.scopes.iter().map(|s| s.as_str()).collect();

    let session =
        AuthorizationSession::new(manager, &scope_refs, &redirect_uri, Some(CLIENT_NAME), None)
            .await
            .map_err(|e| anyhow::anyhow!("OAuth authorization setup failed: {e}"))?;

    Ok(LoginSession {
        auth_url: session.get_authorization_url().to_string(),
        session,
        redirect_port: settings.redirect_port(),
    })
}

impl LoginSession {
    /// Run a one-shot loopback listener to catch the redirect, then exchange the
    /// code for a token (persisted via the credential store). Times out after
    /// `timeout`.
    pub async fn wait_for_callback(self, timeout: Duration) -> anyhow::Result<()> {
        let port = self.redirect_port;
        let captured =
            tokio::task::spawn_blocking(move || listen_for_callback(port, timeout)).await??;

        self.session
            .handle_callback(&captured.code, &captured.state)
            .await
            .map_err(|e| anyhow::anyhow!("OAuth token exchange failed: {e}"))?;
        Ok(())
    }
}

struct CapturedCode {
    code: String,
    state: String,
}

/// Blocking single-request loopback HTTP listener for the OAuth redirect.
fn listen_for_callback(port: u16, timeout: Duration) -> anyhow::Result<CapturedCode> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("cannot bind 127.0.0.1:{port} for OAuth redirect: {e}"))?;
    listener.set_nonblocking(false).ok();

    let deadline = std::time::Instant::now() + timeout;
    // Poll accept with a short read timeout so the overall deadline is honored.
    listener
        .set_nonblocking(true)
        .map_err(|e| anyhow::anyhow!("listener config failed: {e}"))?;

    loop {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for OAuth redirect on port {port}");
        }
        match listener.accept() {
            Ok((mut stream, _addr)) => {
                stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let request_line = read_request_line(&mut stream)?;
                let (code, state) = parse_callback(&request_line)?;
                let body = "<html><body><h3>zerostack: authorization complete.</h3>You can close this tab and return to the terminal.</body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                return Ok(CapturedCode { code, state });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(anyhow::anyhow!("accept failed: {e}")),
        }
    }
}

fn read_request_line(stream: &mut std::net::TcpStream) -> anyhow::Result<String> {
    let mut buf = [0u8; 4096];
    let n = stream
        .read(&mut buf)
        .map_err(|e| anyhow::anyhow!("read redirect request failed: {e}"))?;
    let text = String::from_utf8_lossy(&buf[..n]);
    let first = text
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty redirect request"))?;
    Ok(first.to_string())
}

/// Parse `GET /callback?code=...&state=... HTTP/1.1` and return (code, state).
pub(crate) fn parse_callback(request_line: &str) -> anyhow::Result<(String, String)> {
    let target = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("malformed redirect request line"))?;
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let v = percent_decode(v);
        match k {
            "code" => code = Some(v),
            "state" => state = Some(v),
            "error" => error = Some(v),
            _ => {}
        }
    }

    if let Some(err) = error {
        anyhow::bail!("authorization server returned an error: {err}");
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok((code, state)),
        _ => anyhow::bail!("redirect missing code or state"),
    }
}

/// Minimal percent-decoding for query values (handles `%XX` and `+`).
pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
