use std::fmt;
#[cfg(feature = "memory")]
use std::path::PathBuf;

/// Typed deferred actions previously encoded as `DEFER_*:` error strings.
/// Using `anyhow::Error::new(SlashOutcome)` + `downcast_ref` avoids stringly-typed control flow.
#[derive(Debug, Clone)]
pub enum SlashOutcome {
    DeferCompress {
        instructions: Option<String>,
    },
    DeferInit,
    DeferReview {
        message: String,
    },
    #[cfg(feature = "memory")]
    DeferEditor {
        path: PathBuf,
    },
    #[cfg(feature = "mcp")]
    DeferMcpLogin {
        server: String,
    },
}

impl fmt::Display for SlashOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeferCompress { instructions } => {
                write!(f, "defer compress: {:?}", instructions)
            }
            Self::DeferInit => write!(f, "defer init"),
            Self::DeferReview { message } => write!(f, "defer review: {}", message),
            #[cfg(feature = "memory")]
            Self::DeferEditor { path } => write!(f, "defer editor: {}", path.display()),
            #[cfg(feature = "mcp")]
            Self::DeferMcpLogin { server } => write!(f, "defer mcp login: {}", server),
        }
    }
}

impl std::error::Error for SlashOutcome {}
