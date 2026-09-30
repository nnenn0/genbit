use anyhow::{Result, bail, ensure};
use serde::Serialize;

/// Text written into HTML, XML, and JSON-LD: not blank, and free of characters that XML cannot
/// represent even when escaped and that HTML treats as parse errors.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub(crate) struct PublishableText(String);

impl PublishableText {
    /// `field` names the input in errors, such as `config.toml: title`.
    pub(crate) fn new(text: String, field: &str) -> Result<Self> {
        ensure!(!text.trim().is_empty(), "{field} must not be empty");
        if let Some(character) = text.chars().find(|&character| {
            (character.is_control() && !matches!(character, '\t' | '\n' | '\r'))
                || matches!(character, '\u{fffe}' | '\u{ffff}')
        }) {
            bail!(
                "{field} must not contain control characters: U+{:04X}",
                u32::from(character)
            );
        }
        Ok(Self(text))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::PublishableText;
    use anyhow::{Context, Result};

    #[test]
    fn rejects_blank_text_and_text_that_xml_cannot_represent() -> Result<()> {
        let text = "tab\tline\ncarriage\r and ✓";
        assert_eq!(
            PublishableText::new(text.to_owned(), "title")?.as_str(),
            text
        );
        for (text, expected) in [
            ("", "title must not be empty"),
            (" \n", "title must not be empty"),
            (
                "a\u{1}b",
                "title must not contain control characters: U+0001",
            ),
            (
                "a\u{b}b",
                "title must not contain control characters: U+000B",
            ),
            (
                "a\u{7f}b",
                "title must not contain control characters: U+007F",
            ),
            (
                "a\u{85}b",
                "title must not contain control characters: U+0085",
            ),
            (
                "a\u{fffe}b",
                "title must not contain control characters: U+FFFE",
            ),
        ] {
            let error = PublishableText::new(text.to_owned(), "title")
                .err()
                .with_context(|| format!("accepted {text:?}"))?;
            assert_eq!(error.to_string(), expected);
        }
        Ok(())
    }
}
