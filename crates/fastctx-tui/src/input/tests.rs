use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Input, decode::Decoder};

fn feed(decoder: &mut Decoder, text: &str, out: &mut Vec<Input>) {
    for c in text.chars() {
        decoder.char(c, out);
    }
}

#[test]
fn vt_key_contract_and_queue_boundary() {
    let fixtures = [
        ("\x1b[A", KeyCode::Up, KeyModifiers::NONE),
        ("\x1b[1;6D", KeyCode::Left, KeyModifiers::SHIFT | KeyModifiers::CONTROL),
        ("\x1b[6~", KeyCode::PageDown, KeyModifiers::NONE),
        ("\x1b[Z", KeyCode::BackTab, KeyModifiers::SHIFT),
        ("\x1bOP", KeyCode::F(1), KeyModifiers::NONE),
        ("\x1b\r", KeyCode::Enter, KeyModifiers::ALT),
        ("\x1b[13;2u", KeyCode::Enter, KeyModifiers::SHIFT),
        ("\x1b[27;3;13~", KeyCode::Enter, KeyModifiers::ALT),
        ("\n", KeyCode::Char('j'), KeyModifiers::CONTROL),
        ("\x08", KeyCode::Char('h'), KeyModifiers::CONTROL),
        ("\x1c", KeyCode::Char(char::from(92)), KeyModifiers::CONTROL),
    ];
    for (text, code, modifiers) in fixtures {
        for split in 0..=text.len() {
            let mut decoder = Decoder::default();
            let mut out = Vec::new();
            feed(&mut decoder, &text[..split], &mut out);
            decoder.boundary(true, &mut out);
            feed(&mut decoder, &text[split..], &mut out);
            decoder.boundary(false, &mut out);
            assert!(
                matches!(out.as_slice(), [Input::Key(k)] if *k == KeyEvent::new(code, modifiers)),
                "{text:?}: {out:?}"
            );
        }
    }
    for text in ["\x1b[?1A", "\x1b[999999999999A", "\x1b[1;0A", "\x1b[200x"] {
        let mut decoder = Decoder::default();
        let mut out = Vec::new();
        feed(&mut decoder, text, &mut out);
        assert!(matches!(out.as_slice(), [Input::Sequence(s)] if s == text));
    }
    let mut decoder = Decoder::default();
    let mut out = Vec::new();
    feed(&mut decoder, "\x1b", &mut out);
    decoder.boundary(false, &mut out);
    assert!(matches!(out.as_slice(), [Input::Key(k)] if k.code == KeyCode::Esc));
}

#[test]
fn bracketed_paste_is_atomic_across_every_character_boundary() {
    let mut decoder = Decoder::default();
    let mut out = Vec::new();
    feed(&mut decoder, "\x1b[200~", &mut out);
    let text = "中文，。😀𠮷\r\n\t\x1b[A\x1b[20x\x1b\x1b尾";
    for (index, c) in text.chars().enumerate() {
        decoder.char(c, &mut out);
        decoder.boundary(false, &mut out);
        assert!(out.iter().all(|event| matches!(event, Input::Pasting(n) if *n <= index + 1)));
    }
    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    decoder.key(key, &mut out);
    for c in "\x1b[201~".chars() {
        decoder.char(c, &mut out);
        decoder.boundary(false, &mut out);
    }
    let delivered: Vec<_> = out.iter().filter(|e| !matches!(e, Input::Pasting(_))).collect();
    assert!(
        matches!(delivered.as_slice(), [Input::Paste(s), Input::Key(k)] if s == &text.replace("\r\n", "\n") && *k == key)
    );
}

#[cfg(windows)]
#[test]
fn console_utf16_repeat_and_native_identity_contract() {
    use super::windows_keys::Keys;
    use windows_sys::Win32::System::Console::KEY_EVENT_RECORD;
    let mut keys = Keys::default();
    let mut decoder = Decoder::default();
    let mut out = Vec::new();
    // KEY_EVENT_RECORD's documented repeat count applies to each UTF-16 unit.
    let mut record: KEY_EVENT_RECORD = unsafe { std::mem::zeroed() };
    record.bKeyDown = 1;
    record.wRepeatCount = 3;
    for unit in "😀".encode_utf16() {
        record.uChar.UnicodeChar = unit;
        keys.record(&record, &mut decoder, &mut out).unwrap();
    }
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|event| matches!(event, Input::Key(k) if k.code == KeyCode::Char('😀'))));
    out.clear();
    record.wRepeatCount = 1;
    record.wVirtualKeyCode = 13;
    record.uChar.UnicodeChar = 13;
    record.dwControlKeyState = 16;
    keys.record(&record, &mut decoder, &mut out).unwrap();
    assert!(matches!(out.as_slice(), [Input::Key(k)] if *k == KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)));
    out.clear();
    record.bKeyDown = 0;
    record.wVirtualKeyCode = 0x12;
    record.dwControlKeyState = 0;
    // Captured from ConPTY: unmappable text commits on Alt release, even
    // inside bracketed paste and for the two halves of an emoji.
    feed(&mut decoder, "\x1b[200~", &mut out);
    for unit in "👋".encode_utf16() {
        record.uChar.UnicodeChar = unit;
        keys.record(&record, &mut decoder, &mut out).unwrap();
    }
    feed(&mut decoder, "\x1b[201~", &mut out);
    assert!(matches!(out.as_slice(), [Input::Paste(s)] if s == "👋"));
    record.bKeyDown = 1;
    record.wVirtualKeyCode = 0;
    record.uChar.UnicodeChar = 0xdc00;
    assert!(keys.record(&record, &mut decoder, &mut out).is_err());
}
