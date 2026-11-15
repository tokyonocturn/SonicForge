//! Windows platform layer for SonicForge.
//!
//! Everything that needs the Windows API lives here so that `sf-core` stays
//! portable and unit testable:
//!
//! * [`endpoints`] — WASAPI endpoint enumeration and default device lookup.
//! * [`apo`] — registration of the SonicForge APO in the Windows audio stack,
//!   including backup/restore of the previous effect chain.
//! * [`paths`] — configuration, log and preset locations.
//! * [`logging`] — small rolling file logger used by all user facing binaries.

pub mod apo;
pub mod endpoints;
pub mod logging;
pub mod paths;

/// CLSID of the SonicForge APO, in registry format.
///
/// Generated once for the project and never reused by another product.
pub const APO_CLSID: &str = "{7E1F4C2A-9B3D-4A86-9F2C-5D8E6A1B7C43}";
/// Friendly name shown by Windows audio tooling.
pub const APO_FRIENDLY_NAME: &str = "SonicForge Audio Processor";
