use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Prompt {
    pub title: String,
    pub text: String,
}

impl Prompt {
    pub fn blank() -> Self {
        Self {
            title: "New".to_owned(),
            text: "Type your prompt here.".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct WindowPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

impl Orientation {
    pub fn toggled(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub window_position: Option<WindowPosition>,
    pub orientation: Orientation,
    pub prompts: Vec<Prompt>,
}

impl Config {
    pub fn add_prompt(&mut self) -> usize {
        self.prompts.push(Prompt::blank());
        self.prompts.len() - 1
    }

    pub fn add_after(&mut self, index: usize) -> usize {
        let insertion_index = index.saturating_add(1).min(self.prompts.len());
        self.prompts.insert(insertion_index, Prompt::blank());
        insertion_index
    }

    pub fn duplicate_prompt(&mut self, index: usize) -> Option<usize> {
        let mut duplicate = self.prompts.get(index)?.clone();
        duplicate.title = format!("{} copy", duplicate.title);
        let insertion_index = index + 1;
        self.prompts.insert(insertion_index, duplicate);
        Some(insertion_index)
    }

    pub fn update_prompt(&mut self, index: usize, title: String, text: String) -> bool {
        let Some(prompt) = self.prompts.get_mut(index) else {
            return false;
        };

        let title = title.trim();
        prompt.title = if title.is_empty() {
            "Untitled".to_owned()
        } else {
            title.to_owned()
        };
        prompt.text = text;
        true
    }

    pub fn delete_prompt(&mut self, index: usize) -> bool {
        if index >= self.prompts.len() {
            return false;
        }
        self.prompts.remove(index);
        true
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            window_position: None,
            orientation: Orientation::Horizontal,
            prompts: vec![
                Prompt {
                    title: "Explain".to_owned(),
                    text: "Explain this clearly and concisely, including any important caveats."
                        .to_owned(),
                },
                Prompt {
                    title: "Review".to_owned(),
                    text: "Review this critically. Identify concrete problems, explain their impact, and suggest focused improvements."
                        .to_owned(),
                },
                Prompt {
                    title: "Rewrite".to_owned(),
                    text: "Rewrite this for clarity and brevity while preserving the original meaning."
                        .to_owned(),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_normalizes_an_empty_title() {
        let mut config = Config::default();

        assert!(config.update_prompt(0, "   ".to_owned(), "body".to_owned()));
        assert_eq!(config.prompts[0].title, "Untitled");
        assert_eq!(config.prompts[0].text, "body");
    }

    #[test]
    fn duplicate_is_inserted_after_the_source() {
        let mut config = Config::default();
        let original = config.prompts[0].clone();

        let index = config.duplicate_prompt(0).expect("source should exist");

        assert_eq!(index, 1);
        assert_eq!(config.prompts[1].text, original.text);
        assert_eq!(config.prompts[1].title, "Explain copy");
    }

    #[test]
    fn deleting_the_last_prompt_is_allowed() {
        let mut config = Config {
            prompts: vec![Prompt::blank()],
            ..Config::default()
        };

        assert!(config.delete_prompt(0));
        assert!(config.prompts.is_empty());
    }

    #[test]
    fn older_config_defaults_to_horizontal() {
        let config: Config =
            serde_json::from_str(r#"{"version":1,"window_position":null,"prompts":[]}"#)
                .expect("old config should still load");

        assert_eq!(config.orientation, Orientation::Horizontal);
    }
}
