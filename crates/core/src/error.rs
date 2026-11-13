use std::fmt;
use std::path::PathBuf;

/// Errors produced by the core crate.
#[derive(Debug)]
pub enum Error {
    /// I/O failure, with the path involved.
    Io(PathBuf, std::io::Error),
    /// Malformed data (desc file, config, ...).
    Parse(String),
    /// Unsupported format (e.g. xz-compressed sync db).
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "{}: {}", p.display(), e),
            Error::Parse(s) => write!(f, "parse error: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn io_err(path: &std::path::Path, e: std::io::Error) -> Error {
    Error::Io(path.to_path_buf(), e)
}
