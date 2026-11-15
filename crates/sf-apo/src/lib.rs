//! SonicForge Audio Processing Object.
//!
//! This DLL is loaded by `audiodg.exe` when an endpoint has SonicForge
//! registered as its endpoint effect. It implements the three interfaces Windows
//! requires from a system effect:
//!
//! * `IAudioProcessingObject` — registration, latency, initialisation.
//! * `IAudioProcessingObjectConfiguration` — the engine is prepared here, never
//!   on the real-time thread.
//! * `IAudioProcessingObjectRT` — the real-time processing callback.
//!
//! The real-time callback never allocates, logs, touches the file system or
//! blocks on a lock: it copies the input to the output buffer and processes it
//! with `try_lock`, so a concurrent configuration reload simply results in one
//! block of pass-through audio instead of a dropout.
//!
//! The crate is Windows-only by construction (it is a COM server), so on other
//! targets it compiles to an empty library and the workspace still lints and
//! tests on Linux. The shipping artefact name is `SonicForgeAPO.dll`: Cargo
//! names the output after the library target, so the packaging steps in
//! `sf-setup`, CI and `packaging/installer.iss` pin that name.

#![cfg(windows)]

pub mod engine;

pub use engine::{ApoEngine, ReloadReport};

#[cfg(windows)]
mod com;

#[cfg(windows)]
pub use com::APO_CLSID_GUID;
