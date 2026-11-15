//! Registration of the SonicForge APO in the Windows audio stack.
//!
//! Windows discovers endpoint effects through the per-endpoint `FxProperties`
//! key. SonicForge registers itself as the *endpoint effect* (EFX) of an
//! endpoint and keeps a backup of whatever was configured before, so that
//! disabling restores the machine exactly as it was.
//!
//! This module only touches the registry. It never deletes another product's
//! CLSID registration and never modifies endpoints the user did not select.

use crate::endpoints::{Endpoint, Flow};
use crate::{APO_CLSID, APO_FRIENDLY_NAME};
use windows::core::GUID;

/// The endpoint effect CLSID property. The GUID is the Windows audio property
/// set GUID; `,5` selects `PKEY_FX_EndpointEffectClsid`.
pub const PKEY_FX_ENDPOINT_EFFECT_CLSID: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5";
/// `PKEY_AudioEndpoint_Disable_SysFx`; must be 0 for enhancements to run.
pub const PKEY_AUDIO_ENDPOINT_DISABLE_SYSFX: &str = "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5";
/// `PKEY_FX_Association`.
pub const PKEY_FX_ASSOCIATION: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},0";

const BACKUP_ROOT: &str = r"SOFTWARE\SonicForge\Backup";
const AUDIO_ENGINE_APOS: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\AudioEngine\AudioProcessingObjects";

/// State of one endpoint with respect to SonicForge.
#[derive(Clone, Debug, PartialEq)]
pub struct EndpointStatus {
    pub endpoint: Endpoint,
    /// CLSID currently configured as the endpoint effect, if any.
    pub effect_clsid: Option<String>,
    /// Whether that CLSID is SonicForge.
    pub enabled: bool,
    /// Whether a backup exists (so disabling can restore the old chain).
    pub has_backup: bool,
}

/// Where the endpoint lives in the registry.
fn endpoint_key(flow: Flow, guid: &str) -> String {
    let branch = match flow {
        Flow::Render => "Render",
        Flow::Capture => "Capture",
    };
    format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\{branch}\{{{guid}}}")
}

fn backup_key(flow: Flow, guid: &str) -> String {
    let branch = match flow {
        Flow::Render => "Render",
        Flow::Capture => "Capture",
    };
    format!(r"{BACKUP_ROOT}\{branch}\{guid}")
}

#[cfg(windows)]
mod reg {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW,
        RegQueryValueExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A key opened for reading and writing, closed on drop.
    pub struct Key(pub HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    impl Key {
        pub fn open(path: &str) -> Result<Self, String> {
            let sub = wide(path);
            let mut handle = HKEY::default();
            let status = unsafe {
                RegOpenKeyExW(
                    HKEY_LOCAL_MACHINE,
                    PCWSTR(sub.as_ptr()),
                    0,
                    KEY_READ | KEY_WRITE,
                    &mut handle,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(format!(
                    "cannot open HKLM\\{path} (error {}). Administrator rights are required.",
                    status.0
                ));
            }
            Ok(Key(handle))
        }

        pub fn create(path: &str) -> Result<Self, String> {
            let sub = wide(path);
            let mut handle = HKEY::default();
            let status = unsafe {
                RegCreateKeyExW(
                    HKEY_LOCAL_MACHINE,
                    PCWSTR(sub.as_ptr()),
                    0,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    None,
                    &mut handle,
                    None,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(format!(
                    "cannot create HKLM\\{path} (error {}). Administrator rights are required.",
                    status.0
                ));
            }
            Ok(Key(handle))
        }

        pub fn get_string(&self, name: &str) -> Option<String> {
            let value = wide(name);
            let mut ty = REG_SZ;
            let mut size = 0u32;
            let status = unsafe {
                RegQueryValueExW(
                    self.0,
                    PCWSTR(value.as_ptr()),
                    None,
                    Some(&mut ty),
                    None,
                    Some(&mut size),
                )
            };
            if status != ERROR_SUCCESS || size == 0 {
                return None;
            }
            let mut buffer = vec![0u8; size as usize];
            let status = unsafe {
                RegQueryValueExW(
                    self.0,
                    PCWSTR(value.as_ptr()),
                    None,
                    Some(&mut ty),
                    Some(buffer.as_mut_ptr()),
                    Some(&mut size),
                )
            };
            if status != ERROR_SUCCESS {
                return None;
            }
            match ty {
                REG_SZ => {
                    let units: Vec<u16> = buffer
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .take_while(|&v| v != 0)
                        .collect();
                    Some(String::from_utf16_lossy(&units))
                }
                _ => None,
            }
        }

        pub fn get_u32(&self, name: &str) -> Option<u32> {
            let value = wide(name);
            let mut ty = REG_DWORD;
            let mut data = [0u8; 4];
            let mut size = 4u32;
            let status = unsafe {
                RegQueryValueExW(
                    self.0,
                    PCWSTR(value.as_ptr()),
                    None,
                    Some(&mut ty),
                    Some(data.as_mut_ptr()),
                    Some(&mut size),
                )
            };
            if status != ERROR_SUCCESS || ty != REG_DWORD {
                return None;
            }
            Some(u32::from_le_bytes(data))
        }

        pub fn set_string(&self, name: &str, value: &str) -> Result<(), String> {
            let key = wide(name);
            let text = wide(value);
            let bytes: Vec<u8> = text.iter().flat_map(|u| u.to_le_bytes()).collect();
            let status =
                unsafe { RegSetValueExW(self.0, PCWSTR(key.as_ptr()), 0, REG_SZ, Some(&bytes)) };
            if status != ERROR_SUCCESS {
                return Err(format!(
                    "cannot write registry value {name} (error {})",
                    status.0
                ));
            }
            Ok(())
        }

        pub fn set_u32(&self, name: &str, value: u32) -> Result<(), String> {
            let key = wide(name);
            let bytes = value.to_le_bytes();
            let status =
                unsafe { RegSetValueExW(self.0, PCWSTR(key.as_ptr()), 0, REG_DWORD, Some(&bytes)) };
            if status != ERROR_SUCCESS {
                return Err(format!(
                    "cannot write registry value {name} (error {})",
                    status.0
                ));
            }
            Ok(())
        }

        pub fn delete_value(&self, name: &str) -> Result<(), String> {
            let key = wide(name);
            let status = unsafe { RegDeleteValueW(self.0, PCWSTR(key.as_ptr())) };
            if status != ERROR_SUCCESS {
                return Err(format!(
                    "cannot delete registry value {name} (error {})",
                    status.0
                ));
            }
            Ok(())
        }

        pub fn delete_tree(path: &str) -> Result<(), String> {
            let sub = wide(path);
            let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(sub.as_ptr())) };
            if status != ERROR_SUCCESS {
                return Err(format!("cannot delete HKLM\\{path} (error {})", status.0));
            }
            Ok(())
        }
    }
}

/// Installs the COM class registration for the APO DLL.
#[cfg(windows)]
pub fn register_server(dll_path: &std::path::Path) -> Result<(), String> {
    let dll = dll_path
        .canonicalize()
        .map_err(|e| format!("cannot resolve '{}': {e}", dll_path.display()))?;
    let dll_text = dll.to_string_lossy().to_string();
    let clsid_key = format!(r"SOFTWARE\Classes\CLSID\{APO_CLSID}");

    {
        let key = reg::Key::create(&clsid_key)?;
        key.set_string("", APO_FRIENDLY_NAME)?;
    }
    {
        let key = reg::Key::create(&format!(r"{clsid_key}\InprocServer32"))?;
        key.set_string("", &dll_text)?;
        key.set_string("ThreadingModel", "Both")?;
    }
    // Registering with the audio engine lets audiodg load the APO. Windows only
    // loads APOs listed under this key.
    {
        let key = reg::Key::create(&format!(r"{AUDIO_ENGINE_APOS}\{APO_CLSID}"))?;
        key.set_string("FriendlyName", APO_FRIENDLY_NAME)?;
        key.set_string("Copyright", "SonicForge contributors, GPL-2.0-or-later")?;
        key.set_u32("MajorVersion", 1)?;
        key.set_u32("MinorVersion", 0)?;
    }
    Ok(())
}

/// Removes the COM class registration.
#[cfg(windows)]
pub fn unregister_server() -> Result<(), String> {
    let clsid_key = format!(r"SOFTWARE\Classes\CLSID\{APO_CLSID}");
    let mut errors = Vec::new();
    if let Err(e) = reg::Key::delete_tree(&clsid_key) {
        errors.push(e);
    }
    if let Err(e) = reg::Key::delete_tree(&format!(r"{AUDIO_ENGINE_APOS}\{APO_CLSID}")) {
        errors.push(e);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Enables SonicForge on one endpoint, backing up the previous effect chain.
#[cfg(windows)]
pub fn enable_endpoint(endpoint: &Endpoint) -> Result<(), String> {
    let base = endpoint_key(endpoint.flow, &endpoint.guid);
    let fx_path = format!(r"{base}\FxProperties");
    let props_path = format!(r"{base}\Properties");

    // 1. Back up the current values once, before the first modification.
    let backup = reg::Key::create(&backup_key(endpoint.flow, &endpoint.guid))?;
    let fx = reg::Key::create(&fx_path)?;
    if backup.get_string("BackedUp").is_none() {
        match fx.get_string(PKEY_FX_ENDPOINT_EFFECT_CLSID) {
            Some(previous) => backup.set_string("PreviousEffectClsid", &previous)?,
            None => backup.set_string("PreviousEffectClsid", "")?,
        }
        backup.set_string("BackedUp", "1")?;
    }
    let props = reg::Key::create(&props_path)?;
    if backup.get_string("BackedUpSysFx").is_none() {
        match props.get_u32(PKEY_AUDIO_ENDPOINT_DISABLE_SYSFX) {
            Some(previous) => backup.set_u32("PreviousDisableSysFx", previous)?,
            None => backup.set_u32("PreviousDisableSysFx", 0xFFFF_FFFF)?,
        }
        backup.set_string("BackedUpSysFx", "1")?;
    }

    // 2. Point the endpoint effect at SonicForge and make sure enhancements
    //    are not disabled for this endpoint.
    fx.set_string(PKEY_FX_ENDPOINT_EFFECT_CLSID, APO_CLSID)?;
    props.set_u32(PKEY_AUDIO_ENDPOINT_DISABLE_SYSFX, 0)?;
    backup.set_string("Installed", "1")?;
    Ok(())
}

/// Disables SonicForge on one endpoint and restores the previous effect chain.
#[cfg(windows)]
pub fn disable_endpoint(endpoint: &Endpoint) -> Result<(), String> {
    let base = endpoint_key(endpoint.flow, &endpoint.guid);
    let fx_path = format!(r"{base}\FxProperties");
    let props_path = format!(r"{base}\Properties");
    let backup_path = backup_key(endpoint.flow, &endpoint.guid);

    // If the endpoint was never touched by us, leave it alone entirely.
    let Ok(backup) = reg::Key::open(&backup_path) else {
        return Ok(());
    };
    if backup.get_string("Installed").is_none() {
        return Ok(());
    }

    if let Ok(fx) = reg::Key::open(&fx_path) {
        // Only remove the value when it still points at SonicForge.
        if fx.get_string(PKEY_FX_ENDPOINT_EFFECT_CLSID).as_deref() == Some(APO_CLSID) {
            match backup.get_string("PreviousEffectClsid") {
                Some(previous) if !previous.is_empty() => {
                    fx.set_string(PKEY_FX_ENDPOINT_EFFECT_CLSID, &previous)?;
                }
                _ => {
                    let _ = fx.delete_value(PKEY_FX_ENDPOINT_EFFECT_CLSID);
                }
            }
        }
    }
    if let (Ok(props), Some(previous)) = (
        reg::Key::open(&props_path),
        backup.get_u32("PreviousDisableSysFx"),
    ) {
        if previous == 0xFFFF_FFFF {
            let _ = props.delete_value(PKEY_AUDIO_ENDPOINT_DISABLE_SYSFX);
        } else {
            let _ = props.set_u32(PKEY_AUDIO_ENDPOINT_DISABLE_SYSFX, previous);
        }
    }
    let _ = reg::Key::delete_tree(&backup_path);
    Ok(())
}

/// Reads the current registration state of an endpoint.
#[cfg(windows)]
pub fn endpoint_status(endpoint: &Endpoint) -> EndpointStatus {
    let base = endpoint_key(endpoint.flow, &endpoint.guid);
    let effect = reg::Key::open(&format!(r"{base}\FxProperties"))
        .ok()
        .and_then(|k| k.get_string(PKEY_FX_ENDPOINT_EFFECT_CLSID));
    let has_backup = reg::Key::open(&backup_key(endpoint.flow, &endpoint.guid))
        .ok()
        .and_then(|k| k.get_string("Installed"))
        .is_some();
    EndpointStatus {
        enabled: effect
            .as_deref()
            .map(|v| v.eq_ignore_ascii_case(APO_CLSID))
            .unwrap_or(false),
        effect_clsid: effect,
        has_backup,
        endpoint: endpoint.clone(),
    }
}

/// Lists every endpoint of a flow with its SonicForge state.
#[cfg(windows)]
pub fn status(flow: Flow) -> Result<Vec<EndpointStatus>, String> {
    let endpoints = crate::endpoints::list(flow)?;
    Ok(endpoints.iter().map(endpoint_status).collect())
}

/// The APO CLSID as a parsed GUID.
pub fn clsid_guid() -> GUID {
    // Parsed once at startup from the canonical string.
    let text = APO_CLSID.trim_matches(|c| c == '{' || c == '}');
    let mut parts = text.split('-');
    let data1 = u32::from_str_radix(parts.next().unwrap_or("0"), 16).unwrap_or(0);
    let data2 = u16::from_str_radix(parts.next().unwrap_or("0"), 16).unwrap_or(0);
    let data3 = u16::from_str_radix(parts.next().unwrap_or("0"), 16).unwrap_or(0);
    let mut data4 = [0u8; 8];
    let tail: String = parts.collect::<Vec<_>>().join("");
    for (i, byte) in data4.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&tail[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    GUID::from_values(data1, data2, data3, data4)
}

/// True when the current process can write to HKLM.
#[cfg(windows)]
pub fn has_admin_rights() -> bool {
    reg::Key::open(r"SOFTWARE\SonicForge").is_ok()
        || reg::Key::create(r"SOFTWARE\SonicForge").is_ok()
}

#[cfg(not(windows))]
pub fn register_server(_dll: &std::path::Path) -> Result<(), String> {
    Err("APO registration is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn unregister_server() -> Result<(), String> {
    Err("APO registration is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn enable_endpoint(_endpoint: &Endpoint) -> Result<(), String> {
    Err("APO registration is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn disable_endpoint(_endpoint: &Endpoint) -> Result<(), String> {
    Err("APO registration is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn status(_flow: Flow) -> Result<Vec<EndpointStatus>, String> {
    Err("APO registration is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn endpoint_status(endpoint: &Endpoint) -> EndpointStatus {
    EndpointStatus {
        endpoint: endpoint.clone(),
        effect_clsid: None,
        enabled: false,
        has_backup: false,
    }
}

#[cfg(not(windows))]
pub fn has_admin_rights() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_keys_use_the_right_branch() {
        let render = endpoint_key(Flow::Render, "abc");
        assert!(render.contains(r"MMDevices\Audio\Render\{abc}"));
        assert!(!render.ends_with(r"\FxProperties"));
    }

    #[test]
    fn backup_keys_are_separate_per_flow() {
        assert_ne!(
            backup_key(Flow::Render, "abc"),
            backup_key(Flow::Capture, "abc")
        );
    }

    #[test]
    fn property_keys_match_windows_audio_definitions() {
        assert_eq!(
            PKEY_FX_ENDPOINT_EFFECT_CLSID,
            "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5"
        );
        assert_eq!(
            PKEY_AUDIO_ENDPOINT_DISABLE_SYSFX,
            "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5"
        );
    }
}
