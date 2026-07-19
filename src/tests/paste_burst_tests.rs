use crate::ui::{EnterVerdict, PasteBurst, is_paste_newline_key};
use crossterm::event::{KeyCode, KeyModifiers};

#[test]
fn paste_newline_keys() {
    // Bare Enter (Windows conhost injects VK_RETURN for pasted newlines).
    assert!(is_paste_newline_key(KeyCode::Enter, KeyModifiers::NONE));
    // Ctrl+J is how crossterm reports a raw pasted '\n' on Unix in raw mode.
    assert!(is_paste_newline_key(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL
    ));
    // Deliberate key combinations are not paste newlines.
    assert!(!is_paste_newline_key(KeyCode::Enter, KeyModifiers::SHIFT));
    assert!(!is_paste_newline_key(KeyCode::Enter, KeyModifiers::ALT));
    assert!(!is_paste_newline_key(
        KeyCode::Char('j'),
        KeyModifiers::NONE
    ));
    assert!(!is_paste_newline_key(
        KeyCode::Char('a'),
        KeyModifiers::CONTROL
    ));
    assert!(!is_paste_newline_key(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT
    ));
}

#[test]
fn genuine_enter_is_submit() {
    let mut burst = PasteBurst::default();
    assert_eq!(burst.on_enter(false), EnterVerdict::Submit);
    assert_eq!(burst.on_enter(false), EnterVerdict::Submit);
}

#[test]
fn enter_with_pending_input_starts_burst() {
    let mut burst = PasteBurst::default();
    // First pasted newline: more input is queued right behind the Enter.
    assert_eq!(burst.on_enter(true), EnterVerdict::Newline);
    // Mid-burst Enters are newlines even when nothing is momentarily queued
    // (the trailing newline of a paste must not submit).
    assert_eq!(burst.on_enter(false), EnterVerdict::Newline);
    assert_eq!(burst.on_enter(false), EnterVerdict::Newline);
}

#[test]
fn burst_ends_after_input_goes_quiet() {
    let mut burst = PasteBurst::default();
    assert_eq!(burst.on_enter(true), EnterVerdict::Newline);
    burst.on_timeout();
    assert_eq!(burst.on_enter(false), EnterVerdict::Submit);
}

#[test]
fn wait_timeout_shortens_during_burst() {
    let mut burst = PasteBurst::default();
    let idle = burst.wait_timeout();
    assert_eq!(burst.on_enter(true), EnterVerdict::Newline);
    let active = burst.wait_timeout();
    assert!(active < idle);
    burst.on_timeout();
    assert_eq!(burst.wait_timeout(), idle);
}
