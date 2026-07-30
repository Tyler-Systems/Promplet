use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::model::Config;

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn default_location() -> Result<Self, String> {
        let local_app_data = env::var_os("LOCALAPPDATA")
            .ok_or_else(|| "Windows did not provide a LOCALAPPDATA directory.".to_owned())?;
        Ok(Self {
            path: PathBuf::from(local_app_data)
                .join("Promplet")
                .join("promplets.json"),
        })
    }

    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<Config, String> {
        if !self.path.exists() {
            return Ok(Config::default());
        }

        let contents = fs::read_to_string(&self.path)
            .map_err(|error| format!("Could not read {}: {error}", self.path.display()))?;
        serde_json::from_str(&contents)
            .map_err(|error| format!("Could not parse {}: {error}", self.path.display()))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&self, config: &Config) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "The settings file has no parent directory.".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;

        let json = serde_json::to_vec_pretty(config)
            .map_err(|error| format!("Could not serialize settings: {error}"))?;
        write_file_atomically(&self.path, &json)
            .map_err(|error| format!("Could not write {}: {error}", self.path.display()))
    }
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let mut temporary_name = file_name.to_os_string();
    temporary_name.push(".tmp");
    let temporary_path = path.with_file_name(temporary_name);

    let result = (|| {
        let mut file = fs::File::create(&temporary_path)?;
        file.write_all(contents)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn settings_round_trip() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "promplet-store-test-{}-{unique}",
            std::process::id()
        ));
        let store = ConfigStore::at(directory.join("promplets.json"));
        let expected = Config::default();

        store.save(&expected).expect("settings should save");
        let actual = store.load().expect("settings should load");

        assert_eq!(actual, expected);
        fs::remove_dir_all(directory).expect("temporary settings should be removable");
    }

    #[test]
    fn saving_replaces_the_existing_file_and_removes_the_temporary_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "promplet-store-replace-test-{}-{unique}",
            std::process::id()
        ));
        let store = ConfigStore::at(directory.join("promplets.json"));
        store
            .save(&Config::default())
            .expect("initial settings should save");

        let mut expected = Config::default();
        expected.prompts[0].title = "Updated".to_owned();
        store.save(&expected).expect("settings should be replaced");

        assert_eq!(store.load().expect("settings should load"), expected);
        assert!(!directory.join("promplets.json.tmp").exists());
        fs::remove_dir_all(directory).expect("temporary settings should be removable");
    }
}
