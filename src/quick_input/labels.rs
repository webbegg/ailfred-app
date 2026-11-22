//! What the input says in words: its field's hint, its log line, a shortcut's symbols.

/// The field's hint: what it's for, given `shots` attached; with none, how to take a screenshot
/// with the capture shortcut `chord`, if one is set.
pub fn hint(shots: usize, chord: Option<&str>) -> String {
    match (shots, chord) {
        (0, Some(chord)) => format!("{}  {chord} to take a screenshot", placeholder(0)),
        _ => placeholder(shots).to_owned(),
    }
}

/// What the log says of a message sent: its size, never its text (the log is a file on disk).
pub fn sent_log(text: &str, pngs: &[Vec<u8>]) -> String {
    let bytes: usize = pngs.iter().map(Vec::len).sum();
    format!("Ailfred: quick input: {} chars with {} capture(s), {bytes} bytes of PNG", text.chars().count(), pngs.len())
}

/// A shortcut's keycaps (`Shortcut::keycaps`, as Settings shows them) as one label: `⇧⌘Space`.
pub fn chord_symbols(keycaps: &[String]) -> String {
    keycaps.concat()
}

pub(super) fn placeholder(shots: usize) -> &'static str {
    match shots {
        0 => "Ask anything…",
        1 => "Ask about this screenshot…",
        _ => "Ask about these screenshots…",
    }
}
