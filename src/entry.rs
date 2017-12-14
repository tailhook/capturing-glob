use std::str::from_utf8;
use std::path::{Path, PathBuf};
use std::ffi::OsStr;

/// Entry that contains file path as well as all capture groups if any
#[derive(Debug)]
pub struct Entry {
    path: PathBuf,
    groups: Vec<(usize, usize)>,
}

impl Entry {
    pub(crate) fn new(path: PathBuf) -> Entry {
        Entry {
            path,
            groups: Vec::new(),
        }
    }
    /// Get path represented by this entry
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Get capture group number `n`
    #[cfg(windows)]
    pub fn group(&self, n: usize) -> Option<&OsStr> {
        self.group_windows(n)
    }
    #[cfg_attr(windows, allow(dead_code))]
    fn group_windows(&self, n: usize) -> Option<&OsStr> {
        if let Some(&(a, b)) = self.groups.get(n) {
            let bytes = self.path.to_str().unwrap().as_bytes();
            Some(Path::new(from_utf8(&bytes[a..b]).unwrap()).as_os_str())
        } else {
            None
        }
    }
    /// Get capture group number `n`
    #[cfg(unix)]
    fn group(&self, n: usize) -> Option<&OsStr> {
        use std::os::unix::ffi::OsStrExt;
        if let Some(&(a, b)) = self.groups.get(n) {
            let bytes = self.path.as_os_str().as_bytes();
            Some(OsStr::from_bytes(&bytes[a..b]))
        } else {
            None
        }
    }
}

impl Into<PathBuf> for Entry {
    fn into(self) -> PathBuf {
        self.path
    }
}

impl AsRef<Path> for Entry {
    fn as_ref(&self) -> &Path {
        self.path.as_ref()
    }
}
