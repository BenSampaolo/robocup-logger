#[cfg(not(test))]
use std::process::Command;
use std::{
    io,
    path::{Path, PathBuf},
};

fn directory(path: &Path) -> io::Result<PathBuf> {
    let path = path.canonicalize()?;
    if path.is_dir() {
        Ok(path)
    } else if path.is_file() {
        path.parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| io::Error::other("Log path has no parent"))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected a log file or directory",
        ))
    }
}

#[cfg(not(test))]
pub(crate) fn open(path: &Path) -> io::Result<()> {
    let directory = directory(path)?;
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    let output = Command::new(program).arg(directory).output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Cannot open log folder: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[cfg(test)]
pub(crate) fn open(path: &Path) -> io::Result<()> {
    directory(path)?;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Desktop launch disabled in unit tests",
    ))
}
