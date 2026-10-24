//! Media attachments referenced from the conversation transcript.
//!
//! An attachment is stored as a *reference* — path plus MIME type — not as
//! bytes. The bytes are read from disk when a provider request is built, so
//! the session file stays small and an attachment survives a restart.

// The helpers are used by the feature-gated `multimodal` module; without that
// feature they are only reachable from the persisted session file.
#![allow(dead_code)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What a media attachment carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Audio,
    Document,
}

impl MediaKind {
    /// The kind implied by a MIME type, `None` when the type is not media
    /// this agent can attach.
    pub fn from_mime(mime: &str) -> Option<Self> {
        if mime.starts_with("image/") {
            Some(Self::Image)
        } else if mime.starts_with("audio/") {
            Some(Self::Audio)
        } else if mime == "application/pdf" {
            Some(Self::Document)
        } else {
            None
        }
    }

    /// The kind's name, for messages shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Document => "document",
        }
    }
}

/// A media attachment recorded on a user message.
///
/// Only the path and MIME type are stored: the bytes are re-read from disk
/// when the transcript is turned into a provider request. `extracted_text`
/// holds the text pulled out of a document at attach time (PDFs), so the file
/// is parsed once instead of on every request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    pub path: PathBuf,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
}

impl MediaRef {
    pub fn new(path: PathBuf, mime: impl Into<String>) -> Self {
        Self {
            path,
            mime: mime.into(),
            extracted_text: None,
        }
    }

    /// What this attachment carries, `None` when the MIME type is unknown.
    pub fn kind(&self) -> Option<MediaKind> {
        MediaKind::from_mime(&self.mime)
    }
}
