//! A data directory has one writer for the entire application lifetime.

#[cfg(windows)]
pub struct StorageLease {
    _file: std::fs::File,
}

#[cfg(windows)]
impl super::Storage {
    pub fn acquire_lease(&self) -> std::io::Result<StorageLease> {
        use std::os::windows::fs::OpenOptionsExt;

        // Windows releases the exclusive handle even after a crash. Keep the
        // file itself: deleting/recreating a lock file would race other starts.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(self.config_path().with_file_name("instance.lock"))?;
        Ok(StorageLease { _file: file })
    }
}
