//! This module contains the implementation details of `bafomet`.
//!
//! By default, it is hidden to the user, unless explicitly enabled
//! with the feature flag `expose_impl`.

pub mod async_runtime;
pub mod collections;
pub mod communication;
pub mod consensus;
pub mod core;
pub mod crypto;
pub mod cst;
pub mod error;
pub mod executable;
pub mod globals;
pub mod ordering;
pub mod prng;
pub mod sync;
pub mod threadpool;
pub mod timeouts;

use std::ops::Drop;

use error::*;
use globals::Flag;

static INITIALIZED: Flag = Flag::new();

/// Configure the init process of the library.
pub struct InitConfig {
    /// Number of threads used by the async runtime.
    pub async_threads: usize,
}

/// Handle to the global data.
///
/// When dropped, the data is deinitialized.
pub struct InitGuard;

/// Initializes global data.
///
/// Should always be called before other methods, otherwise runtime
/// panics may ensue.
pub unsafe fn init(c: InitConfig) -> Result<Option<InitGuard>> {
    if INITIALIZED.test() {
        return Ok(None);
    }
    async_runtime::init(c.async_threads)?;
    communication::socket::init()?;
    INITIALIZED.set();
    Ok(Some(InitGuard))
}

impl Drop for InitGuard {
    fn drop(&mut self) {
        unsafe { drop().unwrap() }
    }
}

unsafe fn drop() -> Result<()> {
    INITIALIZED.unset();
    async_runtime::drop()?;
    communication::socket::drop()?;
    Ok(())
}
