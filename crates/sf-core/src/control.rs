//! Live control block shared between the GUI, the tray application and the APO.
//!
//! The audio processing thread must never touch the file system or allocate, so
//! live adjustments (bypass, master preamp, configuration generation) are
//! exchanged through a small memory mapped structure. The APO polls the
//! generation counters; when the configuration generation changes it reloads
//! the configuration on a worker thread and swaps the pipeline between blocks.

/// Name of the shared memory object. `Local\` scopes it to the session.
pub const SHARED_NAME: &str = "Local\\SonicForgeControl";

const MAGIC: u32 = 0x5346_4F52; // "SFOR"
const VERSION: u32 = 1;

/// Point in time snapshot of the control block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControlSnapshot {
    /// Incremented on every write.
    pub generation: u32,
    /// When true the APO must pass audio through untouched.
    pub bypass: bool,
    /// Master preamp in dB applied after the configured chain.
    pub preamp_db: f32,
    /// Incremented by editors after writing a new configuration file.
    pub config_generation: u32,
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
        MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
    };

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct ControlData {
        pub magic: u32,
        pub version: u32,
        pub generation: u32,
        pub bypass: u32,
        pub preamp_millidb: i32,
        pub config_generation: u32,
        pub reserved: [u32; 12],
    }

    impl Default for ControlData {
        fn default() -> Self {
            Self {
                magic: MAGIC,
                version: VERSION,
                generation: 0,
                bypass: 0,
                preamp_millidb: 0,
                config_generation: 0,
                reserved: [0; 12],
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Mapped view of the control structure.
    pub struct Mapping {
        handle: HANDLE,
        view: *mut ControlData,
    }

    // The mapping is process-wide shared memory guarded by volatile accesses;
    // sending the handle across threads is sound.
    unsafe impl Send for Mapping {}
    unsafe impl Sync for Mapping {}

    impl Mapping {
        pub fn create() -> std::io::Result<Self> {
            let name = wide(SHARED_NAME);
            let handle = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE,
                    0,
                    std::mem::size_of::<ControlData>() as u32,
                    name.as_ptr(),
                )
            };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mapped = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
            let view = mapped.Value as *mut ControlData;
            if view.is_null() {
                unsafe { CloseHandle(handle) };
                return Err(std::io::Error::last_os_error());
            }
            let mapping = Self { handle, view };
            if unsafe { (*view).magic } != MAGIC || unsafe { (*view).version } != VERSION {
                unsafe {
                    std::ptr::write_volatile(view, ControlData::default());
                }
            }
            Ok(mapping)
        }

        pub fn open() -> std::io::Result<Self> {
            let name = wide(SHARED_NAME);
            let handle = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mapped = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
            let view = mapped.Value as *mut ControlData;
            if view.is_null() {
                unsafe { CloseHandle(handle) };
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self { handle, view })
        }

        pub fn read(&self) -> ControlSnapshot {
            unsafe {
                let generation = std::ptr::read_volatile(&(*self.view).generation);
                let bypass = std::ptr::read_volatile(&(*self.view).bypass) != 0;
                let preamp = std::ptr::read_volatile(&(*self.view).preamp_millidb) as f32 / 1000.0;
                let config_generation = std::ptr::read_volatile(&(*self.view).config_generation);
                ControlSnapshot {
                    generation,
                    bypass,
                    preamp_db: preamp,
                    config_generation,
                }
            }
        }

        pub fn write(&self, snapshot: ControlSnapshot) {
            unsafe {
                std::ptr::write_volatile(&mut (*self.view).bypass, u32::from(snapshot.bypass));
                std::ptr::write_volatile(
                    &mut (*self.view).preamp_millidb,
                    (snapshot.preamp_db * 1000.0).round() as i32,
                );
                std::ptr::write_volatile(
                    &mut (*self.view).config_generation,
                    snapshot.config_generation,
                );
                let next = std::ptr::read_volatile(&(*self.view).generation).wrapping_add(1);
                std::ptr::write_volatile(&mut (*self.view).generation, next);
            }
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.view as *mut core::ffi::c_void,
                });
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn cell() -> &'static Mutex<super::ControlSnapshot> {
        static CELL: OnceLock<Mutex<ControlSnapshot>> = OnceLock::new();
        CELL.get_or_init(|| Mutex::new(ControlSnapshot::default()))
    }

    /// In-process stand-in used by unit tests on non-Windows hosts.
    pub struct Mapping;

    impl Mapping {
        pub fn create() -> std::io::Result<Self> {
            Ok(Mapping)
        }

        pub fn open() -> std::io::Result<Self> {
            Ok(Mapping)
        }

        pub fn read(&self) -> ControlSnapshot {
            *cell().lock().unwrap()
        }

        pub fn write(&self, mut snapshot: ControlSnapshot) {
            let mut guard = cell().lock().unwrap();
            snapshot.generation = guard.generation.wrapping_add(1);
            *guard = snapshot;
        }
    }
}

pub use platform::Mapping;

/// Convenience wrapper around a shared mapping.
pub struct ControlBlock {
    mapping: Mapping,
}

impl ControlBlock {
    /// Creates (or opens) the control block, initialising it when new.
    pub fn create() -> std::io::Result<Self> {
        Ok(Self {
            mapping: Mapping::create()?,
        })
    }

    /// Opens an existing control block.
    pub fn open() -> std::io::Result<Self> {
        Ok(Self {
            mapping: Mapping::open()?,
        })
    }

    pub fn read(&self) -> ControlSnapshot {
        self.mapping.read()
    }

    pub fn write(&self, snapshot: ControlSnapshot) {
        self.mapping.write(snapshot)
    }

    /// Convenience: toggles bypass and returns the new snapshot.
    pub fn set_bypass(&self, bypass: bool) -> ControlSnapshot {
        let mut snap = self.read();
        snap.bypass = bypass;
        self.write(snap);
        self.read()
    }

    /// Convenience: sets the master preamp in dB and returns the new snapshot.
    pub fn set_preamp_db(&self, db: f32) -> ControlSnapshot {
        let mut snap = self.read();
        snap.preamp_db = db.clamp(-60.0, 24.0);
        self.write(snap);
        self.read()
    }

    /// Signals that the configuration file changed.
    pub fn bump_config(&self) -> ControlSnapshot {
        let mut snap = self.read();
        snap.config_generation = snap.config_generation.wrapping_add(1);
        self.write(snap);
        self.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_block_round_trips_values() {
        let block = ControlBlock::create().unwrap();
        block.write(ControlSnapshot {
            generation: 0,
            bypass: false,
            preamp_db: 0.0,
            config_generation: 3,
        });
        let snap = block.set_bypass(true);
        assert!(snap.bypass);
        let snap = block.set_preamp_db(-6.5);
        assert!((snap.preamp_db + 6.5).abs() < 1e-3);
        assert!(snap.generation > 0);
        assert_eq!(snap.config_generation, 3);
    }

    #[test]
    fn preamp_is_clamped() {
        let block = ControlBlock::create().unwrap();
        assert_eq!(block.set_preamp_db(100.0).preamp_db, 24.0);
        assert_eq!(block.set_preamp_db(-200.0).preamp_db, -60.0);
    }

    #[test]
    fn config_generation_increments() {
        let block = ControlBlock::create().unwrap();
        let start = block.read().config_generation;
        let next = block.bump_config();
        assert_eq!(next.config_generation, start.wrapping_add(1));
    }
}
