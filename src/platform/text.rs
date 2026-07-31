//! Conversion of prompt text into platform-neutral keyboard units.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextUnit {
    Return,
    Unicode(u16),
}

pub fn plan_text(text: &str) -> Vec<TextUnit> {
    let mut units = Vec::with_capacity(text.len());
    let mut characters = text.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                units.push(TextUnit::Return);
            }
            '\n' => units.push(TextUnit::Return),
            character => {
                let mut encoded = [0_u16; 2];
                units.extend(
                    character
                        .encode_utf16(&mut encoded)
                        .iter()
                        .copied()
                        .map(TextUnit::Unicode),
                );
            }
        }
    }

    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_plan_preserves_unicode_surrogate_pairs() {
        assert_eq!(
            plan_text("A😀"),
            vec![
                TextUnit::Unicode('A' as u16),
                TextUnit::Unicode(0xD83D),
                TextUnit::Unicode(0xDE00),
            ]
        );
    }

    #[test]
    fn text_plan_normalizes_all_newline_styles() {
        assert_eq!(
            plan_text("a\r\nb\rc\nd"),
            vec![
                TextUnit::Unicode('a' as u16),
                TextUnit::Return,
                TextUnit::Unicode('b' as u16),
                TextUnit::Return,
                TextUnit::Unicode('c' as u16),
                TextUnit::Return,
                TextUnit::Unicode('d' as u16),
            ]
        );
    }
}
