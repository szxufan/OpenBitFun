//! Desktop wiring for the shared native Computer Use provider.
pub use openbitfun_computer_use::NativeComputerUseHost as DesktopComputerUseHost;
pub(crate) use openbitfun_computer_use::{
    control_session, grants, permissions, share_grants, share_targets, shared_preview,
};

/// Startup registration of the bundled Linux OCR tessdata directory, consumed
/// by `lib.rs` (crate root) and read back by the shared crate's Linux backend.
#[cfg(target_os = "linux")]
pub(crate) use openbitfun_computer_use::register_bundled_tessdata_dir;

#[cfg(all(feature = "devtools", target_os = "macos"))]
#[path = "native_control_roundtrip_tests.rs"]
pub(crate) mod native_control_roundtrip;
