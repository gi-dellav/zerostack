#![cfg(feature = "multimodal")]

use crate::extras::multimodal::{MediaAttachment, detect_media, load_attachment};
use std::path::Path;

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

// --- load_attachment tests ---

#[test]
fn load_attachment_file_not_found() {
    let err = load_attachment(Path::new("/nonexistent/file.png")).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn load_attachment_unknown_media_type() {
    let err = load_attachment(Path::new("Cargo.toml")).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn load_attachment_success_for_small_media() {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let path = dir.join("zerostack_test_media.png");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"fake png data").unwrap();
    drop(f);
    let result = load_attachment(&path);
    let _ = std::fs::remove_file(&path);
    assert!(result.is_ok(), "expected Ok, got {result:?}");
    let att = result.unwrap();
    assert_eq!(att.size(), 13);
    assert_eq!(att.path().to_string_lossy(), path.to_string_lossy());
}

// --- MediaAttachment size and path ---

#[test]
fn media_attachment_size_matches_data_len() {
    let att = MediaAttachment::Image {
        path: Path::new("test.png").to_path_buf(),
        data: vec![0u8; 42],
        mime: "image/png".into(),
    };
    assert_eq!(att.size(), 42);
}

#[test]
fn media_attachment_path_returns_stored_path() {
    let att = MediaAttachment::Audio {
        path: Path::new("/tmp/sound.wav").to_path_buf(),
        data: vec![0u8; 10],
        mime: "audio/wav".into(),
    };
    assert_eq!(att.path(), Path::new("/tmp/sound.wav"));
}

// --- media_to_messages tests ---

#[cfg(feature = "multimodal")]
#[test]
fn media_to_messages_produces_user_messages() {
    use crate::agent::runner::media_to_messages;
    use rig::completion::Message;

    let media = vec![
        MediaAttachment::Image {
            path: Path::new("photo.png").to_path_buf(),
            data: vec![1, 2, 3],
            mime: "image/png".into(),
        },
        MediaAttachment::Document {
            path: Path::new("doc.pdf").to_path_buf(),
            data: vec![4, 5, 6],
            mime: "application/pdf".into(),
        },
    ];

    let messages = media_to_messages(&media);
    assert_eq!(messages.len(), 2);

    for msg in &messages {
        assert!(
            matches!(msg, Message::User { .. }),
            "expected User message, got {msg:?}"
        );
    }
}

#[cfg(feature = "multimodal")]
#[test]
fn media_to_messages_empty_vec_returns_empty() {
    use crate::agent::runner::media_to_messages;

    let messages = media_to_messages(&[]);
    assert!(messages.is_empty());
}
