use crate::{navigation::Keybindings, storage::atomic_write, teams::team_number};
use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub log_directory: PathBuf,
    pub python: PathBuf,
    pub model_cache: PathBuf,
    pub event: String,
    pub home: String,
    pub away: String,
    pub robots: [String; 5],
    pub keys: Keybindings,
}

impl Default for Settings {
    fn default() -> Self {
        let speech = PathBuf::from(env::var_os("HOME").unwrap_or_default())
            .join("Programming/phillip-neutts-german");
        Self {
            log_directory: "logs".into(),
            python: speech.join(".venv/bin/python"),
            model_cache: speech.join("models/hub"),
            event: String::new(),
            home: "HULKs".into(),
            away: String::new(),
            robots: Default::default(),
            keys: Keybindings::default(),
        }
    }
}

impl Settings {
    pub fn path() -> io::Result<PathBuf> {
        let directory = env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "Cannot locate settings: neither XDG_CONFIG_HOME nor HOME is set",
                )
            })?;
        Ok(directory.join("robocup-logger/settings.json"))
    }

    pub fn load() -> io::Result<Self> {
        Self::load_from(&Self::path()?)
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&Self::path()?)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (label, path) in [
            ("Log directory", &self.log_directory),
            ("Python interpreter", &self.python),
            ("Model cache", &self.model_cache),
        ] {
            if path.as_os_str().is_empty() {
                return Err(format!("{label} must not be empty"));
            }
            if path.to_string_lossy().chars().any(char::is_control) {
                return Err(format!("{label} must not contain control characters"));
            }
        }
        for (label, value) in [
            ("Event", &self.event),
            ("Tracked team", &self.home),
            ("Opponent", &self.away),
        ]
        .into_iter()
        .chain(self.robots.iter().map(|robot| ("Robot name", robot)))
        {
            if value.chars().any(char::is_control) {
                return Err(format!("{label} must not contain control characters"));
            }
        }
        let tracked = team_number(&self.home).ok_or("Choose a known tracked team")?;
        if !self.away.trim().is_empty() {
            let opponent = team_number(&self.away).ok_or("Choose a known opponent")?;
            if tracked == opponent {
                return Err("Tracked team and opponent must be different".into());
            }
        }
        self.keys.validate()
    }

    fn load_from(path: &Path) -> io::Result<Self> {
        if !regular_file_exists(path)? {
            return Ok(Self::default());
        }
        let contents = fs::read(path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Reading settings {}: {error}", path.display()),
            )
        })?;
        let settings: Self =
            serde_json::from_slice(&contents).map_err(|error| invalid_settings(path, error))?;
        settings
            .validate()
            .map_err(|error| invalid_settings(path, error))?;
        Ok(settings)
    }

    fn save_to(&self, path: &Path) -> io::Result<()> {
        self.validate()
            .map_err(|error| invalid_settings(path, error))?;
        let replace = regular_file_exists(path)?;
        if replace {
            Self::load_from(path)?;
        }
        let contents =
            serde_json::to_vec_pretty(self).map_err(|error| invalid_settings(path, error))?;
        let directory = path
            .parent()
            .ok_or_else(|| invalid_settings(path, "Path has no parent"))?;
        fs::create_dir_all(directory).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "Creating settings directory {}: {error}",
                    directory.display()
                ),
            )
        })?;
        atomic_write(path, &contents, replace)
    }
}

fn invalid_settings(path: &Path, error: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Invalid settings {}: {error}", path.display()),
    )
}

fn regular_file_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(invalid_settings(
            path,
            "Expected a regular file, not a symlink or directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("Inspecting settings {}: {error}", path.display()),
        )),
    }
}

#[cfg(test)]
#[path = "../tests/unit/settings.rs"]
mod tests;
