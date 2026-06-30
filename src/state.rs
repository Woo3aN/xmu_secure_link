use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::{fs, path::PathBuf};

/// Per-user data directory holding the saved SSO session and device id.
pub fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "MySecureLinkRs").context("cannot resolve data dir")?;
    let path = dirs.data_local_dir().to_path_buf();
    fs::create_dir_all(&path)?;
    Ok(path)
}
