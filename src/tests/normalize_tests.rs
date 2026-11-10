use crate::agent::tools::normalize::{
    NormalizedText, levenshtein_similarity, normalize_whitespace,
};

#[test]
fn normalize_tabs_to_spaces() {
    assert_eq!(
        normalize_whitespace("\tfn foo() {\n\t    bar\n\t}\n"),
        "    fn foo() {\n        bar\n    }\n"
    );
}

#[test]
fn normalize_trailing_spaces() {
    assert_eq!(normalize_whitespace("hello   \nworld\n"), "hello\nworld\n");
}

#[test]
fn normalize_collapse_blank_lines() {
    assert_eq!(normalize_whitespace("a\n\n\nb\n"), "a\n\nb\n");
}

#[test]
fn normalized_text_matches_plain_normalization() {
    let cases = [
        "",
        "hello",
        "hello\n",
        "\tfn foo() {\n\t    bar\n\t}\n",
        "hello   \nworld\n",
        "a\n\n\nb\n",
        "a\r\nb\r\n",
        "a  \r\nb\r\n",
        "a\n\n\n",
    ];
    for case in cases {
        assert_eq!(
            NormalizedText::new(case).text,
            normalize_whitespace(case),
            "mapped normalization differs for {case:?}"
        );
    }
}

#[test]
fn normalized_text_maps_trimmed_trailing_whitespace() {
    let content = "# Release notes  \nVersion: 1.4  \nAuthor: dana  \n\nStatus: draft  \nOwner: bob\nReviewed: no\n";
    let normalized = NormalizedText::new(content);
    let search = normalize_whitespace("Status: draft\nOwner: bob");
    let pos = normalized.text.find(&search).unwrap();
    let (start, end) = normalized.original_range(pos, search.len()).unwrap();
    assert_eq!(&content[start..end], "Status: draft  \nOwner: bob\n");
    assert_eq!((start, end), (49, 76));
}

#[test]
fn normalized_text_maps_after_collapsed_blank_lines() {
    let content = "a\n\n\n\nb\n";
    let normalized = NormalizedText::new(content);
    let search = normalize_whitespace("b");
    let pos = normalized.text.find(&search).unwrap();
    let (start, end) = normalized.original_range(pos, search.len()).unwrap();
    assert_eq!(&content[start..end], "b\n");
}

#[test]
fn normalized_text_maps_tab_expansion() {
    let content = "\tindent\n\tfoo\nbar\n";
    let normalized = NormalizedText::new(content);
    let search = normalize_whitespace("    foo\nbar");
    let pos = normalized.text.find(&search).unwrap();
    let (start, end) = normalized.original_range(pos, search.len()).unwrap();
    assert_eq!(&content[start..end], "\tfoo\nbar\n");
}

#[test]
fn levenshtein_identical() {
    assert!((levenshtein_similarity("hello", "hello") - 1.0).abs() < 0.001);
}

#[test]
fn levenshtein_similar() {
    let sim = levenshtein_similarity("hello world", "helo world");
    assert!(sim > 0.85, "expected >0.85, got {sim}");
}

#[test]
fn levenshtein_different() {
    let sim = levenshtein_similarity("hello", "zzzzz");
    assert!(sim < 0.4, "expected <0.4, got {sim}");
}
