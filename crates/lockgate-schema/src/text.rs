use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisallowedCharacterKind {
    LineBreak,
    Control,
    Format,
    LineSeparator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayStringViolation {
    Empty,
    DisallowedCharacter { byte_index: usize, character: char },
    TooLong { max_bytes: usize },
}

impl fmt::Display for DisallowedCharacterKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::LineBreak => "line break",
            Self::Control => "control character",
            Self::Format => "format character",
            Self::LineSeparator => "line separator",
        })
    }
}

pub(crate) fn classify_disallowed_character(character: char) -> Option<DisallowedCharacterKind> {
    if matches!(character, '\n' | '\r') {
        Some(DisallowedCharacterKind::LineBreak)
    } else if matches!(character, '\u{2028}' | '\u{2029}') {
        Some(DisallowedCharacterKind::LineSeparator)
    } else if character.is_control() {
        Some(DisallowedCharacterKind::Control)
    } else if is_format_character(character) {
        Some(DisallowedCharacterKind::Format)
    } else {
        None
    }
}

pub(crate) fn find_disallowed_character(
    value: &str,
) -> Option<(usize, char, DisallowedCharacterKind)> {
    value.char_indices().find_map(|(byte_index, character)| {
        classify_disallowed_character(character).map(|kind| (byte_index, character, kind))
    })
}

pub(crate) fn validate_display_string(
    value: &str,
    max_bytes: usize,
    extra_disallowed: impl Fn(char) -> bool,
) -> Result<(), DisplayStringViolation> {
    let mut has_non_whitespace = false;
    let mut disallowed = None;
    for (byte_index, character) in value.char_indices() {
        has_non_whitespace |= !character.is_whitespace();
        if disallowed.is_none()
            && (classify_disallowed_character(character).is_some() || extra_disallowed(character))
        {
            disallowed = Some((byte_index, character));
        }
    }
    if !has_non_whitespace {
        return Err(DisplayStringViolation::Empty);
    }
    if let Some((byte_index, character)) = disallowed {
        return Err(DisplayStringViolation::DisallowedCharacter {
            byte_index,
            character,
        });
    }
    if value.len() > max_bytes {
        return Err(DisplayStringViolation::TooLong { max_bytes });
    }
    Ok(())
}

// Rust exposes Unicode Cc through `is_control`, but not Cf. Keep this explicit
// table synchronized with Unicode's format-character assignments.
fn is_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}
