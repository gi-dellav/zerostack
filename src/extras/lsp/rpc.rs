//! Minimal JSON-RPC framing for LSP: `Content-Length`-headed messages over
//! the server's stdio. Hand-rolled to keep the dependency tree at `lsp-types`
//! only. Unit-tested over `tokio::io::duplex` — no live server needed.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Writes one framed JSON-RPC message.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, body: &[u8]) -> std::io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    w.write_all(header.as_bytes()).await?;
    w.write_all(body).await?;
    w.flush().await
}

/// Reads one framed JSON-RPC message. Returns `Ok(None)` on a clean EOF
/// before any header byte (server exited); an error on EOF mid-message.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut headers = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        let n = r.read(&mut byte).await?;
        if n == 0 {
            if headers.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof inside LSP headers",
            ));
        }
        headers.push(byte[0]);
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
        if headers.len() > MAX_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "LSP header block too large",
            ));
        }
    }

    let headers = String::from_utf8_lossy(&headers);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("Content-Length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing Content-Length in LSP frame",
            )
        })?;
    if content_length > MAX_BODY_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "LSP message body too large",
        ));
    }

    let mut body = vec![0u8; content_length];
    r.read_exact(&mut body).await?;
    Ok(Some(body))
}
