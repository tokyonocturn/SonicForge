//! WASAPI endpoint enumeration.
//!
//! Uses the MMDevice API, which reports exactly the endpoints the Windows audio
//! engine exposes: the same names, IDs and connection types shown in the Sound
//! control panel.

use crate::logging;

/// Direction of an audio endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Render,
    Capture,
}

impl Flow {
    pub fn as_str(self) -> &'static str {
        match self {
            Flow::Render => "render",
            Flow::Capture => "capture",
        }
    }
}

/// One audio endpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    /// Stable endpoint ID string, e.g. `{0.0.0.00000000}.{guid}`.
    pub id: String,
    /// Device friendly name, e.g. `Speakers (Realtek(R) Audio)`.
    pub name: String,
    /// Connection name, e.g. `Speakers`.
    pub connection: String,
    pub flow: Flow,
    /// True when this is the default endpoint for its flow.
    pub is_default: bool,
    /// Endpoint GUID without braces, lower case.
    pub guid: String,
}

impl Endpoint {
    /// Extracts the GUID part of an MMDevice id such as
    /// `{0.0.0.00000000}.{1a2b3c4d-...}`.
    pub fn guid_from_id(id: &str) -> String {
        match id.rsplit_once('.') {
            Some((_, guid)) => guid.trim_matches(|c| c == '{' || c == '}').to_ascii_lowercase(),
            None => id.trim_matches(|c| c == '{' || c == '}').to_ascii_lowercase(),
        }
    }

    /// Device identity used by the configuration `Device:` command.
    pub fn device_info(&self) -> sf_core::device::DeviceInfo {
        sf_core::device::DeviceInfo {
            name: self.name.clone(),
            connection: self.connection.clone(),
            guid: self.guid.clone(),
            flow: match self.flow {
                Flow::Render => sf_core::device::Flow::Render,
                Flow::Capture => sf_core::device::Flow::Capture,
            },
        }
    }
}

/// Lists all active endpoints of a flow.
#[cfg(windows)]
pub fn list(flow: Flow) -> Result<Vec<Endpoint>, String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
        STGM_READ,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;

    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        if hr.is_err() {
            return Err(format!("CoInitializeEx failed: {hr:?}"));
        }
        let result = (|| -> Result<Vec<Endpoint>, String> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("cannot create MMDeviceEnumerator: {e}"))?;
            let data_flow = match flow {
                Flow::Render => eRender,
                Flow::Capture => eCapture,
            };
            let collection = enumerator
                .EnumAudioEndpoints(data_flow, DEVICE_STATE_ACTIVE)
                .map_err(|e| format!("cannot enumerate endpoints: {e}"))?;
            let default_id = enumerator
                .GetDefaultAudioEndpoint(data_flow, eConsole)
                .ok()
                .and_then(|dev| dev.GetId().ok())
                .and_then(|id| id.to_string().ok())
                .unwrap_or_default();

            let count = collection.GetCount().unwrap_or(0);
            let mut endpoints = Vec::with_capacity(count as usize);
            for index in 0..count {
                let Ok(device) = collection.Item(index) else { continue };
                let Ok(id_ptr) = device.GetId() else { continue };
                let Ok(id) = id_ptr.to_string() else { continue };
                let store = match device.OpenPropertyStore(STGM_READ) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let friendly = store
                    .GetValue(&PKEY_Device_FriendlyName)
                    .ok()
                    .and_then(|v| PropVariantToStringAlloc(&v).ok())
                    .and_then(|p| p.to_string().ok())
                    .unwrap_or_default();
                let desc = store
                    .GetValue(&windows::Win32::Devices::FunctionDiscovery::PKEY_Device_DeviceDesc)
                    .ok()
                    .and_then(|v| PropVariantToStringAlloc(&v).ok())
                    .and_then(|p| p.to_string().ok())
                    .unwrap_or_default();
                let connection = if desc.trim().is_empty() {
                    match friendly.find('(') {
                        Some(idx) => friendly[..idx].trim().to_string(),
                        None => friendly.clone(),
                    }
                } else {
                    desc
                };
                endpoints.push(Endpoint {
                    guid: Endpoint::guid_from_id(&id),
                    is_default: id == default_id,
                    connection,
                    name: friendly,
                    id,
                    flow,
                });
            }
            Ok(endpoints)
        })();
        CoUninitialize();
        if let Err(e) = &result {
            logging::warn(&format!("endpoint enumeration failed: {e}"));
        }
        result
    }
}

/// Finds an endpoint by endpoint id, GUID, or a case insensitive substring of
/// its name or connection. An empty selector returns the default endpoint.
#[cfg(windows)]
pub fn find(flow: Flow, selector: &str) -> Result<Option<Endpoint>, String> {
    let endpoints = list(flow)?;
    let needle = selector.trim().trim_matches('"');
    if needle.is_empty() {
        return Ok(endpoints.into_iter().find(|e| e.is_default));
    }
    let lower = needle.to_lowercase();
    let normalized = needle.trim_matches(|c| c == '{' || c == '}').to_lowercase();
    if let Some(exact) = endpoints
        .iter()
        .find(|e| e.id.eq_ignore_ascii_case(needle) || e.guid == normalized)
    {
        return Ok(Some(exact.clone()));
    }
    Ok(endpoints
        .into_iter()
        .find(|e| e.name.to_lowercase().contains(&lower) || e.connection.to_lowercase().contains(&lower)))
}

/// The default endpoint of a flow, if any.
#[cfg(windows)]
pub fn default_endpoint(flow: Flow) -> Result<Option<Endpoint>, String> {
    Ok(list(flow)?.into_iter().find(|e| e.is_default))
}

#[cfg(not(windows))]
pub fn list(_flow: Flow) -> Result<Vec<Endpoint>, String> {
    Err("audio endpoints are only available on Windows".into())
}

#[cfg(not(windows))]
pub fn find(_flow: Flow, _selector: &str) -> Result<Option<Endpoint>, String> {
    Err("audio endpoints are only available on Windows".into())
}

#[cfg(not(windows))]
pub fn default_endpoint(_flow: Flow) -> Result<Option<Endpoint>, String> {
    Err("audio endpoints are only available on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_guid_from_endpoint_id() {
        assert_eq!(
            Endpoint::guid_from_id("{0.0.0.00000000}.{A1B2C3D4-0000-0000-0000-000000000000}"),
            "a1b2c3d4-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn guid_extraction_handles_unusual_ids() {
        assert_eq!(Endpoint::guid_from_id("plain-guid"), "plain-guid");
        assert_eq!(Endpoint::guid_from_id("{GUID}"), "guid");
    }

    #[test]
    fn device_info_matches_configuration_language() {
        let endpoint = Endpoint {
            id: "{0.0.0.00000000}.{abc}".into(),
            name: "Speakers (Realtek(R) Audio)".into(),
            connection: "Speakers".into(),
            flow: Flow::Render,
            is_default: true,
            guid: "abc".into(),
        };
        let info = endpoint.device_info();
        assert!(sf_core::device::matches_device("Realtek Speakers", &info));
        assert!(!sf_core::device::matches_device("Headphones", &info));
    }
}
