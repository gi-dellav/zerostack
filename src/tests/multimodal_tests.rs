#![cfg(feature = "multimodal")]

use crate::extras::multimodal::{
    ProviderSupport, attach, detect_media, user_contents, validate_for_provider,
};
use crate::media::{MediaKind, MediaRef};
use std::path::{Path, PathBuf};

// --- detect_media tests ---

#[test]
fn detect_media_image_extensions() {
    assert_eq!(detect_media(Path::new("photo.png")), Some("image/png"));
    assert_eq!(detect_media(Path::new("photo.jpg")), Some("image/jpeg"));
    assert_eq!(detect_media(Path::new("photo.jpeg")), Some("image/jpeg"));
    assert_eq!(detect_media(Path::new("photo.GIF")), Some("image/gif"));
    assert_eq!(detect_media(Path::new("photo.webp")), Some("image/webp"));
}

#[test]
fn detect_media_audio_extensions() {
    assert_eq!(detect_media(Path::new("song.mp3")), Some("audio/mpeg"));
    assert_eq!(detect_media(Path::new("song.wav")), Some("audio/wav"));
    assert_eq!(detect_media(Path::new("song.ogg")), Some("audio/ogg"));
    assert_eq!(detect_media(Path::new("song.flac")), Some("audio/flac"));
    assert_eq!(detect_media(Path::new("song.m4a")), Some("audio/mp4"));
    assert_eq!(detect_media(Path::new("song.aac")), Some("audio/aac"));
}

#[test]
fn detect_media_document_extension() {
    assert_eq!(detect_media(Path::new("doc.pdf")), Some("application/pdf"));
}

#[test]
fn detect_media_unknown_returns_none() {
    assert_eq!(detect_media(Path::new("code.rs")), None);
    assert_eq!(detect_media(Path::new("README.md")), None);
    assert_eq!(detect_media(Path::new("script.sh")), None);
    assert_eq!(detect_media(Path::new("Dockerfile")), None);
    assert_eq!(detect_media(Path::new("data.txt")), None);
}

#[test]
fn detect_media_no_extension_returns_none() {
    assert_eq!(detect_media(Path::new("Makefile")), None);
    assert_eq!(detect_media(Path::new("/usr/bin/binary")), None);
}

// --- helpers ---

/// A scratch file under the system temp dir. The name carries the extension
/// the attachment logic keys on, and the process id so two concurrent runs
/// do not share a file.
fn scratch(name: &str, bytes: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("zerostack_test_{}_{name}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

// --- attach tests ---

#[test]
fn attach_file_not_found() {
    let err = attach(Path::new("/nonexistent/file.png")).unwrap_err();
    assert!(err.to_string().contains("No such file"), "got {err}");
}

#[test]
fn attach_unknown_media_type() {
    let path = scratch("attach_unknown.txt", b"not media");
    let err = attach(&path).unwrap_err();
    let _ = std::fs::remove_file(&path);
    assert!(
        err.to_string().contains("unsupported media type"),
        "got {err}"
    );
}

#[test]
fn attach_records_path_and_mime() {
    let path = scratch("attach_ok.png", b"fake png data");
    let media = attach(&path).unwrap();
    assert_eq!(media.mime, "image/png");
    assert_eq!(media.kind(), Some(MediaKind::Image));
    assert_eq!(
        media.path.canonicalize().unwrap(),
        path.canonicalize().unwrap()
    );
    assert!(media.extracted_text.is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
#[cfg(not(feature = "pdf"))]
fn attach_pdf_without_pdf_feature_is_refused() {
    let path = scratch("attach_pdf.pdf", b"%PDF-1.4");
    let err = attach(&path).unwrap_err();
    let _ = std::fs::remove_file(&path);
    assert!(err.to_string().contains("pdf` feature"), "got {err}");
}

#[test]
fn attach_follows_the_content_when_the_extension_lies() {
    let path = scratch("attach_jpeg_as_png.png", b"\xFF\xD8\xFF\xe0 fake jpeg");
    let media = attach(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(media.mime, "image/jpeg");
}

#[test]
fn attach_refuses_content_of_another_kind() {
    let path = scratch("attach_pdf_as_png.png", b"%PDF-1.4 not an image");
    let err = attach(&path).unwrap_err();
    let _ = std::fs::remove_file(&path);
    assert!(
        err.to_string().contains("file content is application/pdf"),
        "got {err}"
    );
}

// --- validate_for_provider tests ---

/// A provider whose transport carries audio (or not).
fn support(name: &'static str, audio: bool) -> ProviderSupport {
    ProviderSupport { name, audio }
}

#[test]
fn validate_rejects_audio_when_the_transport_cannot_carry_it() {
    let media = MediaRef::new(PathBuf::from("sound.wav"), "audio/wav");
    let err = validate_for_provider(support("anthropic", false), &media).unwrap_err();
    assert!(
        err.to_string().contains("does not accept audio"),
        "got {err}"
    );
}

#[test]
fn validate_accepts_audio_when_the_transport_carries_it() {
    let media = MediaRef::new(PathBuf::from("sound.wav"), "audio/wav");
    assert!(validate_for_provider(support("gemini", true), &media).is_ok());
}

/// Ollama's API has no audio input, so an audio attachment is refused there
/// even for a model that can hear: the bytes have no field to travel in.
#[test]
fn validate_rejects_audio_on_ollama() {
    let media = MediaRef::new(PathBuf::from("sound.wav"), "audio/wav");
    let err = validate_for_provider(support("ollama", false), &media).unwrap_err();
    assert!(
        err.to_string().contains("ollama does not accept audio"),
        "got {err}"
    );
}

#[test]
fn validate_rejects_oversized_image_for_anthropic() {
    let bytes = vec![0u8; 6 * 1024 * 1024];
    let path = scratch("attach_big.png", &bytes);
    let media = attach(&path).unwrap();
    let err = validate_for_provider(support("anthropic", true), &media).unwrap_err();
    let _ = std::fs::remove_file(&path);
    assert!(err.to_string().contains("too large"), "got {err}");
    // The same file is fine for a provider without a smaller cap.
    assert!(validate_for_provider(support("openai", true), &media).is_ok());
}

// --- user_contents tests ---

#[test]
fn user_contents_for_image_is_base64() {
    use base64::Engine as _;
    use rig::completion::message::DocumentSourceKind;
    use rig::completion::message::UserContent;

    let bytes = b"fake png bytes";
    let path = scratch("contents_image.png", bytes);
    let media = attach(&path).unwrap();

    let contents = user_contents(std::slice::from_ref(&media));
    let _ = std::fs::remove_file(&path);
    assert_eq!(contents.len(), 1);
    let UserContent::Image(image) = &contents[0] else {
        panic!("expected an image content block, got {:?}", contents[0]);
    };
    // Raw bytes are rejected by every provider; the payload must be base64.
    let DocumentSourceKind::Base64(payload) = &image.data else {
        panic!("expected base64 data, got {:?}", image.data);
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .unwrap();
    assert_eq!(decoded, bytes);
    assert_eq!(
        image
            .media_type
            .as_ref()
            .map(|m| rig::completion::message::MimeType::to_mime_type(m)),
        Some("image/png")
    );
}

#[test]
fn user_contents_for_missing_file_is_skipped() {
    let media = MediaRef::new(PathBuf::from("/nonexistent/gone.png"), "image/png");
    assert!(user_contents(std::slice::from_ref(&media)).is_empty());
}

#[test]
fn user_contents_empty_returns_empty() {
    assert!(user_contents(&[]).is_empty());
}

// --- UserPrompt tests ---

#[test]
fn prompt_without_media_is_a_single_text_block() {
    use rig::completion::Message;
    use rig::completion::message::UserContent;

    let message = crate::agent::runner::UserPrompt::text("hello").to_message();
    let Message::User { content } = message else {
        panic!("expected a user message");
    };
    assert_eq!(content.len(), 1);
    assert!(matches!(content[0], UserContent::Text(_)));
}

/// The attachment must survive the provider's own conversion: this is what
/// an OpenAI-shaped request carries on the wire.
#[test]
fn prompt_image_converts_to_the_openai_wire_form() {
    use rig::completion::Message;
    use rig::providers::openai::completion::UserContent as OpenAiUserContent;

    let bytes = b"fake png bytes";
    let path = scratch("wire_image.png", bytes);
    let media = attach(&path).unwrap();

    let prompt = crate::agent::runner::UserPrompt {
        text: "look at this".to_string(),
        media: vec![media],
    };
    let Message::User { content } = prompt.to_message() else {
        panic!("expected a user message");
    };

    let converted: Vec<OpenAiUserContent> = content
        .into_iter()
        .map(|c| c.try_into().expect("the provider must accept the content"))
        .collect();
    let _ = std::fs::remove_file(&path);

    assert_eq!(converted.len(), 2);
    let OpenAiUserContent::Image { image_url } = &converted[1] else {
        panic!("expected an image part, got {:?}", converted[1]);
    };
    assert!(
        image_url.url.starts_with("data:image/png;base64,"),
        "got {}",
        image_url.url
    );
}

#[test]
fn prompt_media_shares_the_user_turn_with_the_text() {
    use rig::completion::Message;
    use rig::completion::message::UserContent;

    let bytes = b"fake png bytes";
    let path = scratch("prompt_image.png", bytes);
    let media = attach(&path).unwrap();

    let prompt = crate::agent::runner::UserPrompt {
        text: "look at this".to_string(),
        media: vec![media],
    };
    let message = prompt.to_message();
    let _ = std::fs::remove_file(&path);
    let Message::User { content } = message else {
        panic!("expected a user message");
    };
    // One user turn carrying both: providers that require alternating roles
    // reject two user messages in a row.
    assert_eq!(content.len(), 2);
    assert!(matches!(content[0], UserContent::Text(_)));
    assert!(matches!(content[1], UserContent::Image(_)));
}
