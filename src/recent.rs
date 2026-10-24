use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{
    settings::Settings,
    storage::{atomic_write, regular_file_exists},
};

const LIMIT: usize = 8;

fn path() -> io::Result<PathBuf> {
    Ok(Settings::path()?.with_file_name("recent_sessions.json"))
}

pub(crate) fn list() -> io::Result<Vec<PathBuf>> {
    list_from(&path()?)
}

fn list_from(path: &Path) -> io::Result<Vec<PathBuf>> {
    if !regular_file_exists(path)? {
        return Ok(Vec::new());
    }
    let paths: Vec<PathBuf> = serde_json::from_slice(&fs::read(path)?).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid recent sessions: {error}"),
        )
    })?;
    let mut result = Vec::new();
    for path in paths {
        if !path.is_absolute() || path.extension().is_none_or(|extension| extension != "json") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Recent sessions must be absolute JSON paths",
            ));
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                if result.len() < LIMIT && !result.contains(&path) {
                    result.push(path);
                }
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(result)
}

pub(crate) fn remember(session: &Path) -> io::Result<()> {
    remember_at(&path()?, session)
}

fn remember_at(path: &Path, session: &Path) -> io::Result<()> {
    let session = session.canonicalize()?;
    if !regular_file_exists(&session)?
        || session
            .extension()
            .is_none_or(|extension| extension != "json")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected a session JSON file",
        ));
    }
    let mut paths = list_from(path)?;
    paths.retain(|old| old != &session);
    paths.insert(0, session);
    paths.truncate(LIMIT);
    let replace = regular_file_exists(path)?;
    let bytes = serde_json::to_vec_pretty(&paths).map_err(io::Error::other)?;
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("Recent sessions path has no parent"))?,
    )?;
    atomic_write(path, &bytes, replace)
}
