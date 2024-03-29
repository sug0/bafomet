//! Error related business logic of `bafomet`.
//!
//! Contains the `ErrorKind` enum generated by the `build.rs` build
//! script, as well as other useful extensions of the `std::result::Result`
//! type, to work with our very own `Error` type.

use std::error;
use std::fmt;
use std::io;
use std::result;

/// Extension of the standard library's `Result` type,
/// used to wrap its error in a `bafomet::error::Error`.
pub trait ResultWrappedExt {
    type T;

    fn wrapped_msg(self, kind: ErrorKind, msg: &str) -> Result<Self::T>;
    fn wrapped(self, kind: ErrorKind) -> Result<Self::T>;
}

/// Extension of the standard library's `Result` type.
///
/// Different from `ResultWrappedExt`, this trait is
/// used in cases where we want to drop the underlying
/// error type in the `Result`. Having this possibility
/// might be useful when the error type in the `Result`
/// doesn't implement `Send`.
pub trait ResultSimpleExt {
    type T;

    fn simple(self, kind: ErrorKind) -> Result<Self::T>;
    fn simple_msg(self, kind: ErrorKind, msg: &str) -> Result<Self::T>;
}

impl<T, E> ResultWrappedExt for result::Result<T, E>
where
    E: Into<Box<dyn error::Error + Send + Sync>>,
{
    type T = T;

    fn wrapped(self, kind: ErrorKind) -> Result<Self::T> {
        self.map_err(|e| Error::wrapped(kind, e))
    }

    fn wrapped_msg(self, kind: ErrorKind, msg: &str) -> Result<Self::T> {
        self.map_err(|e| Error::wrapped(kind, format!("{}: {}", msg, e.into())))
    }
}

impl<T, E> ResultSimpleExt for result::Result<T, E> {
    type T = T;

    fn simple(self, kind: ErrorKind) -> Result<Self::T> {
        self.map_err(|_| Error::simple(kind))
    }

    fn simple_msg(self, kind: ErrorKind, msg: &str) -> Result<Self::T> {
        self.map_err(|_| Error::wrapped(kind, msg))
    }
}

/// Wrapper result type for `std::result::Result`.
pub type Result<T> = result::Result<T, Error>;

/// The error type used throughout this crate.
pub struct Error {
    inner: ErrorInner,
}

#[derive(Debug)]
enum ErrorInner {
    Simple(ErrorKind),
    Wrapped(ErrorKind, Box<dyn error::Error + Send + Sync>),
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

impl Error {
    /// Returns an error not wrapping another
    /// error implementation, with kind `ErrorKind`.
    pub fn simple(kind: ErrorKind) -> Self {
        let inner = ErrorInner::Simple(kind);
        Error { inner }
    }

    /// Wraps an arbitrary error in an `Error`,
    /// with kind of type `ErrorKind`.
    pub fn wrapped<E>(kind: ErrorKind, e: E) -> Self
    where
        E: Into<Box<dyn error::Error + Send + Sync>>,
    {
        let inner = ErrorInner::Wrapped(kind, e.into());
        Error { inner }
    }

    /// Returns a copy of the `ErrorKind` of this `Error`.
    pub fn kind(&self) -> ErrorKind {
        match &self.inner {
            ErrorInner::Simple(k) => *k,
            ErrorInner::Wrapped(k, _) => *k,
        }
    }

    /// Swaps the `ErrorKind` of this `Error`.
    pub fn swap_kind(self, k: ErrorKind) -> Self {
        let inner = match self.inner {
            ErrorInner::Simple(_) => ErrorInner::Simple(k),
            ErrorInner::Wrapped(_, e) => ErrorInner::Wrapped(k, e),
        };
        Error { inner }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            ErrorInner::Simple(k) => write!(fmt, "{:?}", k),
            ErrorInner::Wrapped(k, e) => write!(fmt, "{:?}: {}", k, e),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::wrapped(ErrorKind::Error, e)
    }
}

impl error::Error for Error {}

pub use error_kind::ErrorKind;

mod error_kind {
    include!(concat!(env!("OUT_DIR"), "/error_kind.rs"));
}
