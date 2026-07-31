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
            // Newlines inside default prompts are always written as
            // backslash-newline: Claude Code and shells read that as a line
            // continuation, so clicking a prompt never presses a bare Enter
            // that would submit a terminal's input mid-text.
            prompts: vec![
                Prompt {
                    title: "Onboard".to_owned(),
                    text: "Explore this repo until you understand it: docs, working code, \
                           recent commits. Make no changes. Then report: a two-sentence \
                           summary, current state, anything half-finished, and your top 3 \
                           next steps, ranked, with a one-line reason each."
                        .to_owned(),
                },
                Prompt {
                    title: "Rules".to_owned(),
                    text: "These writing style rules apply to everything you write: replies, \
                           docs, code comments, commit messages, drafted prose.\\\n\
                           - Open with the answer. No preamble, praise, apologies, restating \
                           my question, or background I already know.\\\n\
                           - Banned constructions: \"it's not just X, it's Y\" / \"no X, no Y, \
                           just Z\"; \"in today's fast-paced/digital world\"; \"whether you're \
                           X or Y\"; \"at its core\"; \"imagine a world where\"; \"think of it \
                           as\"; \"let's dive in\" / \"here's the thing\"; \"it's worth \
                           noting\"; \"by doing X, you can Y\"; unsourced \"studies show\" / \
                           \"experts agree\".\\\n\
                           - Banned words (unless quoting or technically required): delve, \
                           tapestry, realm, landscape, ecosystem, seamless, robust, nuanced, \
                           crucial, pivotal, comprehensive, leverage, unlock, elevate, \
                           transform, game-changing.\\\n\
                           - Use concrete nouns, active verbs, names, and numbers. A claim \
                           with no source, number, or named entity behind it gets cut, not \
                           hedged.\\\n\
                           - Don't pad lists to three items. Vary sentence and paragraph \
                           length; no uniform rhythm, no chained fragments.\\\n\
                           - Plain paragraphs by default. Headings and bullets only when \
                           structure materially helps; numbered lists only for true \
                           sequences; no \"**Label**: text\" bullets, emoji bullets, or \
                           decorative arrows.\\\n\
                           - At most one em dash per response.\\\n\
                           - Make the call: recommendation first, caveats after. Don't \
                           manufacture balance or retreat to \"it depends\".\\\n\
                           - Hedge at most once, and only when the uncertainty changes what I \
                           should do. One short disclaimer only where it changes the \
                           answer.\\\n\
                           - No stock transitions (moreover, furthermore, additionally), no \
                           announced conclusions (\"in conclusion\", \"ultimately\"), no \
                           narrating your own structure (\"in this section we'll...\", \
                           mid-answer recaps).\\\n\
                           - End on substance: no summary recap, no \"let me know if you'd \
                           like...\", no menus of generic options. When the work points to a \
                           real next step, name it concretely (the exact command, file, or \
                           decision); one or two, not a list of maybes.\\\n\
                           - Code: give the fix first; explain only the non-obvious parts."
                        .to_owned(),
                },
                Prompt {
                    title: "Paths".to_owned(),
                    text: "Excellent. Suggest some next paths we can work on.".to_owned(),
                },
                Prompt {
                    title: "Wrap".to_owned(),
                    text: "What are we forgetting as we look to wrap the session? Is there \
                           anything I should have asked, or anything from this session that \
                           should be saved?"
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
        assert_eq!(config.prompts[1].title, "Onboard copy");
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
    fn default_prompts_never_press_a_bare_enter() {
        for prompt in Config::default().prompts {
            let mut previous = ' ';
            for character in prompt.text.chars() {
                if character == '\n' {
                    assert_eq!(
                        previous, '\\',
                        "newline in default prompt “{}” lacks a continuation backslash",
                        prompt.title
                    );
                }
                previous = character;
            }
        }
    }

    #[test]
    fn older_config_defaults_to_horizontal() {
        let config: Config =
            serde_json::from_str(r#"{"version":1,"window_position":null,"prompts":[]}"#)
                .expect("old config should still load");

        assert_eq!(config.orientation, Orientation::Horizontal);
    }
}
