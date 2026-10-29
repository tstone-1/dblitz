//! OS-specific glue, one module per platform. Code that must compile and be
//! tested everywhere even though one platform uses it lives in `launch.rs`.

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(windows)]
pub(crate) mod open_with;
#[cfg(windows)]
pub(crate) mod windows;
