use anyhow::{Context, Result, bail};
use fs2::FileExt;
use rand::RngCore;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn default_profile() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("Could not locate the user profile")?;
    // Electron used the roaming configuration directory, not a new Rust app ID.
    Ok(base.config_dir().join("relay-download-manager"))
}
pub fn default_downloads() -> Result<PathBuf> {
    let user = directories::UserDirs::new().context("Could not locate your home directory")?;
    Ok(user.download_dir().unwrap_or(user.home_dir()).join("Relay"))
}
pub struct ProfileLock(File);
impl ProfileLock {
    pub fn acquire(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("relay.lock"))?;
        file.try_lock_exclusive()
            .context("Relay is already using this profile. Close the other instance first.")?;
        Ok(Self(file))
    }
}
impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
pub fn pairing_token(dir: &Path) -> Result<String> {
    let path = dir.join("browser-token");
    if path.exists() {
        let token = fs::read_to_string(&path)?;
        if token.trim().is_empty() {
            bail!(
                "The browser pairing token is empty. Preserve the profile before attempting recovery."
            );
        }
        return Ok(token);
    }
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    Ok(token)
}
pub fn atomic_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let dir = path.parent().context("Missing profile directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    Ok(())
}
