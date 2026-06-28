use std::pin::Pin;
use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    pub max_attempts: usize,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 500,
            max_backoff_ms: 10_000,
        }
    }
}

pub fn simple_jitter(range_ms: u64) -> Duration {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter = nanos % range_ms.max(1);
    Duration::from_millis(jitter)
}

pub fn is_retryable(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = current {
        let msg = e.to_string();

        if msg.contains("429")
            || msg.contains("503")
            || msg.contains("502")
            || msg.contains("504")
            || msg.contains("500")
        {
            return true;
        }

        let lower = msg.to_lowercase();
        if lower.contains("stream ended")
            || lower.contains("connection")
            || lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("reset by peer")
            || lower.contains("broken pipe")
            || lower.contains("dns")
            || lower.contains("rate limit")
            || lower.contains("too many requests")
            || lower.contains("service unavailable")
            || lower.contains("temporarily unavailable")
            || lower.contains("internal server error")
            || lower.contains("bad gateway")
            || lower.contains("gateway timeout")
            || lower.contains("error sending request")
        {
            return true;
        }

        current = e.source();
    }
    false
}

#[allow(dead_code)]
pub async fn with_retry<T, E, F, Fut>(config: &RetryConfig, mut f: F) -> Result<T, E>
where
    E: std::error::Error + 'static,
    Fut: std::future::Future<Output = Result<T, E>>,
    F: FnMut() -> Fut,
{
    let mut attempt: usize = 0;
    let mut backoff = Duration::from_millis(config.initial_backoff_ms);
    let max_backoff = Duration::from_millis(config.max_backoff_ms);

    loop {
        attempt += 1;
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt >= config.max_attempts || !is_retryable(&e) {
                    return Err(e);
                }
                let jitter = simple_jitter(backoff.as_millis() as u64);
                let delay = backoff + jitter;
                tracing::warn!(
                    "retryable error (attempt {attempt}/{}): {e}. Retrying in {}ms...",
                    config.max_attempts,
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
}

pub async fn retry_stream_chat<T, E, Fut, S>(
    config: &RetryConfig,
    mut factory: impl FnMut() -> Fut,
) -> Result<Pin<Box<dyn futures::Stream<Item = Result<T, E>> + Send>>, E>
where
    E: std::error::Error + Send + 'static,
    Fut: std::future::Future<Output = S>,
    S: futures::Stream<Item = Result<T, E>> + Send + Unpin + 'static,
    T: Send + 'static,
{
    let mut attempt: usize = 0;
    let mut backoff = Duration::from_millis(config.initial_backoff_ms);
    let max_backoff = Duration::from_millis(config.max_backoff_ms);

    loop {
        attempt += 1;
        let mut stream = factory().await;
        let first = stream.next().await;
        match first {
            Some(Ok(item)) => {
                return Ok(futures::stream::once(std::future::ready(Ok(item)))
                    .chain(stream)
                    .boxed());
            }
            Some(Err(e)) => {
                if attempt >= config.max_attempts || !is_retryable(&e) {
                    return Err(e);
                }
                let jitter = simple_jitter(backoff.as_millis() as u64);
                let delay = backoff + jitter;
                tracing::warn!(
                    "retryable error on first stream item (attempt {attempt}/{}): {e}",
                    config.max_attempts
                );
                tokio::time::sleep(delay).await;
                backoff = (backoff * 2).min(max_backoff);
            }
            None => return Ok(stream.boxed()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_retry_config_defaults() {
        let cfg = RetryConfig::default();
        assert_eq!(cfg.max_attempts, 3);
        assert_eq!(cfg.initial_backoff_ms, 500);
        assert_eq!(cfg.max_backoff_ms, 10_000);
    }

    #[test]
    fn test_is_retryable_connection_error() {
        let err = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_timeout() {
        let err = io::Error::new(io::ErrorKind::TimedOut, "operation timed out");
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_http_429() {
        let err = io::Error::new(io::ErrorKind::Other, "HTTP 429 Too Many Requests");
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_http_500() {
        let err = io::Error::new(io::ErrorKind::Other, "HTTP 500 Internal Server Error");
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_not_retryable_parse_error() {
        let err = serde_json::Error::io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "unexpected eof",
        ));
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_not_retryable_generic() {
        let err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_walks_source_chain() {
        let inner = io::Error::new(io::ErrorKind::TimedOut, "timed out");
        let outer = io::Error::new(io::ErrorKind::Other, inner);
        assert!(is_retryable(&outer));
    }

    #[tokio::test]
    async fn test_with_retry_succeeds_first_try() {
        let cfg = RetryConfig::default();
        let result = with_retry(&cfg, || async { Ok::<_, io::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_with_retry_fails_immediately_on_non_retryable() {
        let cfg = RetryConfig::default();
        let mut calls = 0;
        let result = with_retry(&cfg, || {
            calls += 1;
            let val = Err::<String, _>(io::Error::new(io::ErrorKind::NotFound, "not found"));
            async move { val }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn test_with_retry_retries_on_retryable() {
        let cfg = RetryConfig {
            max_attempts: 3,
            initial_backoff_ms: 1,
            max_backoff_ms: 10,
        };
        let mut calls = 0;
        let result = with_retry(&cfg, || {
            calls += 1;
            let current = calls;
            async move {
                if current < 3 {
                    Err::<String, _>(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "connection refused",
                    ))
                } else {
                    Ok("success".to_string())
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), "success");
        assert_eq!(calls, 3);
    }

    #[tokio::test]
    async fn test_with_retry_exhausts_attempts() {
        let cfg = RetryConfig {
            max_attempts: 2,
            initial_backoff_ms: 1,
            max_backoff_ms: 10,
        };
        let mut calls = 0;
        let result = with_retry(&cfg, || {
            calls += 1;
            let val = Err::<String, _>(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "connection refused",
            ));
            async move { val }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn test_with_retry_single_attempt_never_retries() {
        let cfg = RetryConfig {
            max_attempts: 1,
            initial_backoff_ms: 1,
            max_backoff_ms: 10,
        };
        let mut calls = 0;
        let result = with_retry(&cfg, || {
            calls += 1;
            let val = Err::<String, _>(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "connection refused",
            ));
            async move { val }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
}
