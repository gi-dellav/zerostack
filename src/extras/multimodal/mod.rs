//! Media attachments: images, audio, and PDFs the user attaches to a message.
//!
//! Images and audio travel to the provider as base64 payloads: every
//! provider rig speaks to rejects raw bytes (`DocumentSourceKind::Raw`).
//! Documents travel as the text extracted from them, which every provider
//! accepts.
//!
//! The attachment and the prompt travel in ONE user message, so providers
//! that require alternating user/assistant roles (Anthropic rejects two user
//! turns in a row) accept the request.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rig::completion::message::{
    Audio, AudioMediaType, DocumentSourceKind, Image, ImageMediaType, MimeType, UserContent,
};

use crate::media::{MediaKind, MediaRef};

/// Largest attachment accepted, in bytes.
pub const MAX_MEDIA_BYTES: u64 = 20 * 1024 * 1024;

/// Largest amount of text extracted from a document, in characters. Bounds
/// how much one attachment can add to the context window.
pub const MAX_EXTRACTED_CHARS: usize = 100_000;

/// Why an attachment could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("{0}")]
    Io(String),
    #[error("unsupported media type: {0}")]
    UnsupportedType(String),
    #[error("file too large: {path} ({size} bytes, max {max} bytes)")]
    TooLarge { path: String, size: u64, max: u64 },
    #[error(
        "{provider} does not accept {kind} attachments: the request format has no {kind} field"
    )]
    UnsupportedKind {
        provider: String,
        kind: &'static str,
    },
    #[error("file content is {found}, but the extension says {expected}")]
    ContentMismatch {
        expected: &'static str,
        found: &'static str,
    },
    #[error("{0}")]
    Extraction(String),
}

/// Check whether a file extension indicates multi-modal media (not text).
/// Returns the MIME type string if recognized, `None` otherwise.
pub fn detect_media(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        "m4a" => Some("audio/mp4"),
        "aac" => Some("audio/aac"),
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

/// Prepare a file for attachment: check it is a media file this agent
/// accepts, and extract the text of a document once so the file is not
/// parsed again on every request.
pub fn attach(path: &Path) -> Result<MediaRef, AttachError> {
    let meta = std::fs::metadata(path).map_err(|e| AttachError::Io(e.to_string()))?;
    if !meta.is_file() {
        return Err(AttachError::Io(format!("not a file: {}", path.display())));
    }
    if meta.len() > MAX_MEDIA_BYTES {
        return Err(AttachError::TooLarge {
            path: path.display().to_string(),
            size: meta.len(),
            max: MAX_MEDIA_BYTES,
        });
    }
    let mime = detect_media(path)
        .ok_or_else(|| AttachError::UnsupportedType(path.display().to_string()))?;
    // The extension says what the file claims to be; the first bytes say
    // what it is. A file whose content is a different kind of media is
    // refused here rather than rejected by the provider later.
    let mime = match sniff_mime(&read_head(path)?) {
        Some(found) if MediaKind::from_mime(found) == MediaKind::from_mime(mime) => found,
        Some(found) => {
            return Err(AttachError::ContentMismatch {
                expected: mime,
                found,
            });
        }
        None => mime,
    };

    let mut media = MediaRef::new(
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
        mime,
    );
    if media.kind() == Some(MediaKind::Document) {
        media.extracted_text = Some(extract_document_text(&media.path)?);
    }
    Ok(media)
}

/// Read the first bytes of a file, for the content sniff.
fn read_head(path: &Path) -> Result<Vec<u8>, AttachError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| AttachError::Io(e.to_string()))?;
    let mut head = vec![0u8; 16];
    let read = file
        .read(&mut head)
        .map_err(|e| AttachError::Io(e.to_string()))?;
    head.truncate(read);
    Ok(head)
}

/// The MIME type implied by the content of a file, `None` when the bytes
/// carry no signature this agent recognizes.
///
/// Only unambiguous signatures are listed: an MP3 frame sync, for instance,
/// also matches AAC and other bitstreams, so it is not used.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"%PDF") {
        return Some("application/pdf");
    }
    if bytes.starts_with(b"ID3") {
        return Some("audio/mpeg");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        return Some("audio/wav");
    }
    if bytes.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    if bytes.starts_with(b"fLaC") {
        return Some("audio/flac");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some("audio/mp4");
    }
    None
}

/// What a provider accepts for attachments.
#[derive(Debug, Clone, Copy)]
pub struct ProviderSupport {
    /// The provider's name, for error messages.
    pub name: &'static str,
    /// Whether the transport can carry an audio attachment. This is a
    /// property of the request format, not of the model: a model that can
    /// hear is unreachable when the format has no field for audio.
    pub audio: bool,
}

/// Check that the provider can take this attachment. Called when the file is
/// attached, so an unsupported combination is refused before a request is
/// built rather than failing inside the provider.
pub fn validate_for_provider(
    support: ProviderSupport,
    media: &MediaRef,
) -> Result<(), AttachError> {
    let kind = media
        .kind()
        .ok_or_else(|| AttachError::UnsupportedType(media.path.display().to_string()))?;
    match kind {
        MediaKind::Image => {
            let max = max_image_bytes(support.name);
            let size = std::fs::metadata(&media.path).map(|m| m.len()).unwrap_or(0);
            if size > max {
                return Err(AttachError::TooLarge {
                    path: media.path.display().to_string(),
                    size,
                    max,
                });
            }
            Ok(())
        }
        MediaKind::Audio => {
            if support.audio {
                Ok(())
            } else {
                Err(AttachError::UnsupportedKind {
                    provider: support.name.to_string(),
                    kind: kind.label(),
                })
            }
        }
        // Documents travel as extracted text, which every provider accepts.
        MediaKind::Document => Ok(()),
    }
}

/// Largest image the provider accepts, in bytes.
fn max_image_bytes(provider: &str) -> u64 {
    match provider {
        // Anthropic rejects base64 images over 5 MB.
        "anthropic" => 5 * 1024 * 1024,
        _ => MAX_MEDIA_BYTES,
    }
}

/// Build the provider content blocks for the given attachments.
///
/// A file that has gone missing is skipped with a warning rather than
/// failing the request: the conversation around it is still valid, and the
/// model is better served by the remaining turns than by no answer at all.
pub fn user_contents(media: &[MediaRef]) -> Vec<UserContent> {
    let mut contents = Vec::with_capacity(media.len());
    for m in media {
        match m.kind() {
            Some(MediaKind::Image) => match read_bytes(m) {
                Ok(bytes) => contents.push(image_content(&bytes, &m.mime)),
                Err(e) => warn_skipped(m, &e),
            },
            Some(MediaKind::Audio) => match read_bytes(m) {
                Ok(bytes) => contents.push(audio_content(&bytes, &m.mime)),
                Err(e) => warn_skipped(m, &e),
            },
            Some(MediaKind::Document) => {
                let text = m
                    .extracted_text
                    .clone()
                    .or_else(|| extract_document_text(&m.path).ok());
                match text {
                    Some(text) => contents.push(UserContent::text(document_text(m, &text))),
                    None => warn_skipped(m, &"document text could not be extracted"),
                }
            }
            None => {}
        }
    }
    contents
}

/// Read the bytes of an attachment.
fn read_bytes(media: &MediaRef) -> Result<Vec<u8>, AttachError> {
    std::fs::read(&media.path).map_err(|e| AttachError::Io(e.to_string()))
}

/// An image as a base64 content block.
fn image_content(bytes: &[u8], mime: &str) -> UserContent {
    UserContent::Image(Image {
        data: DocumentSourceKind::Base64(BASE64.encode(bytes)),
        media_type: ImageMediaType::from_mime_type(mime),
        detail: None,
        additional_params: None,
    })
}

/// Audio as a base64 content block.
fn audio_content(bytes: &[u8], mime: &str) -> UserContent {
    UserContent::Audio(Audio {
        data: DocumentSourceKind::Base64(BASE64.encode(bytes)),
        media_type: audio_media_type(mime),
        additional_params: None,
    })
}

/// The rig audio type for a MIME type. `audio/mpeg` is the standard spelling
/// for MP3, while rig names it `audio/mp3`.
fn audio_media_type(mime: &str) -> Option<AudioMediaType> {
    match mime {
        "audio/mpeg" | "audio/mp3" => Some(AudioMediaType::MP3),
        "audio/wav" => Some(AudioMediaType::WAV),
        "audio/ogg" => Some(AudioMediaType::OGG),
        "audio/flac" => Some(AudioMediaType::FLAC),
        "audio/mp4" => Some(AudioMediaType::M4A),
        "audio/aac" => Some(AudioMediaType::AAC),
        _ => None,
    }
}

/// Label a document's text with the file it came from, so the model can
/// refer to it.
fn document_text(media: &MediaRef, text: &str) -> String {
    let name = media
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| media.path.display().to_string());
    format!("[contents of {name}]\n{text}")
}

/// Extract the text of a document.
#[cfg(feature = "pdf")]
fn extract_document_text(path: &Path) -> Result<String, AttachError> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| AttachError::Extraction(format!("could not read {}: {e}", path.display())))?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    if pages.is_empty() {
        return Err(AttachError::Extraction(format!(
            "{} has no pages",
            path.display()
        )));
    }
    let text = doc.extract_text(&pages).map_err(|e| {
        AttachError::Extraction(format!("could not extract {}: {e}", path.display()))
    })?;
    Ok(truncate_text(text))
}

/// Extract the text of a document.
#[cfg(not(feature = "pdf"))]
fn extract_document_text(path: &Path) -> Result<String, AttachError> {
    Err(AttachError::Extraction(format!(
        "PDF attachments need the `pdf` feature (rebuild with --features multimodal,pdf): {}",
        path.display()
    )))
}

/// Cut extracted text at a character boundary, marking what was dropped.
fn truncate_text(text: String) -> String {
    if text.len() <= MAX_EXTRACTED_CHARS {
        return text;
    }
    let cut = text
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= MAX_EXTRACTED_CHARS)
        .last()
        .unwrap_or(0);
    let mut out = text[..cut].to_string();
    out.push_str("\n\n[truncated]");
    out
}

/// Report an attachment that could not be read.
fn warn_skipped(media: &MediaRef, reason: &impl std::fmt::Display) {
    tracing::warn!(
        "skipping media attachment {}: {reason}",
        media.path.display()
    );
}
