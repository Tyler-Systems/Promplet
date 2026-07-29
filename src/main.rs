#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod model;
mod platform;
mod store;
mod ui;

use fltk::{app, dialog};
use model::Config;
use store::ConfigStore;
use ui::{Editor, Message, Strip};

fn main() {
    if let Err(error) = run() {
        dialog::alert_default(&format!("Promplet could not start:\n\n{error}"));
    }
}

fn run() -> Result<(), String> {
    let application = app::App::default().with_scheme(app::Scheme::Base);
    let (sender, receiver) = app::channel::<Message>();

    let store = ConfigStore::default_location()?;
    let (mut config, load_warning) = match store.load() {
        Ok(config) => (config, None),
        Err(error) => (Config::default(), Some(error)),
    };

    let mut strip = Strip::new(sender);
    let mut editor = Editor::new(sender, strip.window());
    strip.rebuild(&config);
    let saved_position = config.window_position;
    let visible_position = strip.show(saved_position)?;
    if saved_position.is_some() && saved_position != Some(visible_position) {
        config.window_position = Some(visible_position);
        save_or_report(&store, &config);
    }
    strip.start_topmost_keeper();

    if let Some(warning) = load_warning {
        dialog::alert_default(&format!(
            "Promplet could not read its settings, so defaults were loaded.\n\n{warning}"
        ));
    }

    while application.wait() {
        if let Some(message) = receiver.recv() {
            match message {
                Message::Insert(index) => {
                    if let Some(prompt) = config.prompts.get(index)
                        && let Err(error) = platform::insert_text(&prompt.text)
                    {
                        dialog::alert_default(&format!(
                            "Promplet could not insert this prompt.\n\n{error}"
                        ));
                    }
                }
                Message::Edit(index) => {
                    if let Some(prompt) = config.prompts.get(index) {
                        editor.open(index, prompt);
                    }
                }
                Message::Create => {
                    let index = config.add_prompt();
                    save_or_report(&store, &config);
                    strip.rebuild(&config);
                    if let Some(prompt) = config.prompts.get(index) {
                        editor.open(index, prompt);
                    }
                }
                Message::ShowConfig => {
                    if let Err(error) = store
                        .save(&config)
                        .and_then(|_| platform::reveal_file(store.path()))
                    {
                        dialog::alert_default(&format!(
                            "Promplet could not show its config file.\n\n{error}"
                        ));
                    }
                }
                Message::Save { index, title, text } => {
                    if config.update_prompt(index, title, text) {
                        save_or_report(&store, &config);
                        strip.rebuild(&config);
                    }
                    editor.hide();
                }
                Message::Duplicate(index) => {
                    if let Some(new_index) = config.duplicate_prompt(index) {
                        save_or_report(&store, &config);
                        strip.rebuild(&config);
                        if let Some(prompt) = config.prompts.get(new_index) {
                            editor.open(new_index, prompt);
                        }
                    }
                }
                Message::AddAfter(index) => {
                    let new_index = config.add_after(index);
                    save_or_report(&store, &config);
                    strip.rebuild(&config);
                    if let Some(prompt) = config.prompts.get(new_index) {
                        editor.open(new_index, prompt);
                    }
                }
                Message::Delete(index) => {
                    if config.delete_prompt(index) {
                        save_or_report(&store, &config);
                        strip.rebuild(&config);
                    }
                    editor.hide();
                }
                Message::Moved(position) => {
                    config.window_position = Some(position);
                    save_or_report(&store, &config);
                }
                Message::CloseEditor => editor.hide(),
                Message::Quit => break,
            }
        }
    }

    Ok(())
}

fn save_or_report(store: &ConfigStore, config: &Config) {
    if let Err(error) = store.save(config) {
        dialog::alert_default(&format!("Promplet could not save its settings.\n\n{error}"));
    }
}
