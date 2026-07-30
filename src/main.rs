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
    let Some(_instance_guard) = platform::claim_single_instance()? else {
        return Ok(());
    };

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
                        editor.open(index, prompt, config.orientation);
                    }
                }
                Message::Create => {
                    let index = config.add_prompt();
                    rebuild_and_save(&mut strip, &store, &mut config);
                    if let Some(prompt) = config.prompts.get(index) {
                        editor.open(index, prompt, config.orientation);
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
                Message::ReloadConfig => match reload_config(&store, &mut config) {
                    Ok(()) => {
                        editor.hide();
                        strip.rebuild(&config);
                        config.window_position = Some(strip.position());
                    }
                    Err(error) => dialog::alert_default(&format!(
                        "Promplet could not reload its settings.\n\n{error}"
                    )),
                },
                Message::ToggleOrientation => {
                    config.orientation = config.orientation.toggled();
                    rebuild_and_save(&mut strip, &store, &mut config);
                    editor.reposition(config.orientation);
                }
                Message::Save { index, title, text } => {
                    if config.update_prompt(index, title, text) {
                        rebuild_and_save(&mut strip, &store, &mut config);
                    }
                    editor.hide();
                }
                Message::Duplicate(index) => {
                    if let Some(new_index) = config.duplicate_prompt(index) {
                        rebuild_and_save(&mut strip, &store, &mut config);
                        if let Some(prompt) = config.prompts.get(new_index) {
                            editor.open(new_index, prompt, config.orientation);
                        }
                    }
                }
                Message::AddAfter(index) => {
                    let new_index = config.add_after(index);
                    rebuild_and_save(&mut strip, &store, &mut config);
                    if let Some(prompt) = config.prompts.get(new_index) {
                        editor.open(new_index, prompt, config.orientation);
                    }
                }
                Message::Delete(index) => {
                    if config.delete_prompt(index) {
                        rebuild_and_save(&mut strip, &store, &mut config);
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

fn reload_config(store: &ConfigStore, config: &mut Config) -> Result<(), String> {
    let reloaded = store.load()?;
    *config = reloaded;
    Ok(())
}

fn rebuild_and_save(strip: &mut Strip, store: &ConfigStore, config: &mut Config) {
    strip.rebuild(config);
    config.window_position = Some(strip.position());
    save_or_report(store, config);
}

fn save_or_report(store: &ConfigStore, config: &Config) {
    if let Err(error) = store.save(config) {
        dialog::alert_default(&format!("Promplet could not save its settings.\n\n{error}"));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temporary_store(test_name: &str) -> (std::path::PathBuf, ConfigStore) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "promplet-{test_name}-{}-{unique}",
            std::process::id()
        ));
        let store = ConfigStore::at(directory.join("promplets.json"));
        (directory, store)
    }

    #[test]
    fn reload_replaces_current_config() {
        let (directory, store) = temporary_store("reload");
        let mut expected = Config::default();
        expected.prompts[0].title = "Reloaded".to_owned();
        store.save(&expected).expect("replacement should save");

        let mut current = Config::default();
        current.prompts.clear();
        reload_config(&store, &mut current).expect("replacement should load");

        assert_eq!(current, expected);
        fs::remove_dir_all(directory).expect("temporary settings should be removable");
    }

    #[test]
    fn failed_reload_keeps_current_config() {
        let (directory, store) = temporary_store("failed-reload");
        fs::create_dir_all(&directory).expect("temporary directory should be created");
        fs::write(store.path(), b"{not json").expect("malformed settings should be written");

        let mut current = Config::default();
        current.prompts[0].title = "Keep me".to_owned();
        let before = current.clone();

        assert!(reload_config(&store, &mut current).is_err());
        assert_eq!(current, before);
        fs::remove_dir_all(directory).expect("temporary settings should be removable");
    }
}
