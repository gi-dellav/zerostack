//! Tests for MCP tool-result content rendering (multimodal image support).

use rig::completion::message::ImageMediaType;
use rig::message::{DocumentSourceKind, ToolResultContent};

use crate::extras::mcp::tool::render_result;

fn blocks(out: rig::tool::ToolOutput) -> Vec<ToolResultContent> {
    out.into_content()
}

#[test]
fn text_only_result_stays_plain_text() {
    let out = render_result(vec!["status: ok".into()], vec![]);
    let items = blocks(out);
    assert_eq!(items.len(), 1);
    assert!(matches!(&items[0], ToolResultContent::Text(t) if t.text == "status: ok"));
}

#[test]
fn image_result_becomes_multimodal_blocks() {
    let out = render_result(
        vec!["Screenshot saved".into()],
        vec![("image/png".into(), "aGVsbG8=".into())],
    );
    let items = blocks(out);
    assert_eq!(items.len(), 2);

    assert!(matches!(&items[0], ToolResultContent::Text(t) if t.text.contains("Screenshot saved")));
    let ToolResultContent::Image(img) = &items[1] else {
        panic!("second part should be an image");
    };
    assert_eq!(img.media_type, Some(ImageMediaType::PNG));
    assert!(matches!(&img.data, DocumentSourceKind::Base64(d) if d == "aGVsbG8="));
}

#[test]
fn image_only_result_omits_response_text() {
    let out = render_result(vec![], vec![("image/jpeg".into(), "aGk=".into())]);
    let items = blocks(out);
    assert_eq!(items.len(), 1);
    let ToolResultContent::Image(img) = &items[0] else {
        panic!("only part should be an image");
    };
    assert_eq!(img.media_type, Some(ImageMediaType::JPEG));
    assert!(matches!(&img.data, DocumentSourceKind::Base64(d) if d == "aGk="));
}

#[test]
fn multiple_blocks_join_texts_and_keep_all_images() {
    let out = render_result(
        vec!["before".into(), "after".into()],
        vec![
            ("image/png".into(), "dg==".into()),
            ("image/webp".into(), "dw==".into()),
        ],
    );
    let items = blocks(out);
    assert_eq!(items.len(), 3);
    assert!(
        matches!(&items[0], ToolResultContent::Text(t) if t.text.contains("before") && t.text.contains("after"))
    );
    assert!(matches!(items[1], ToolResultContent::Image(_)));
    assert!(matches!(items[2], ToolResultContent::Image(_)));
}

#[test]
fn coaching_text_lands_ahead_of_images() {
    let out = render_result(
        vec!["[note] approved".into(), "result".into()],
        vec![("image/png".into(), "dg==".into())],
    );
    let items = blocks(out);
    assert!(matches!(
        &items[0],
        ToolResultContent::Text(t) if t.text.contains("[note] approved") && t.text.contains("result")
    ));
    assert!(matches!(items[1], ToolResultContent::Image(_)));
}
