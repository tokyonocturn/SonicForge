//! COM plumbing for the SonicForge APO.
//!
//! The COM object is implemented with explicit vtables rather than a derive
//! macro. An APO needs three interfaces with precise vtable layouts and a
//! single reference count shared between them, which is exactly what the
//! classic "interface subobject" layout below provides.
//!
//! ```text
//! ApoObject
//!   vtbl      ─┐  <- IAudioProcessingObject* points here
//!   self       │
//!   cfg_vtbl   │  <- IAudioProcessingObjectConfiguration* points here
//!   cfg_self   │
//!   rt_vtbl    │  <- IAudioProcessingObjectRT* points here
//!   rt_self    │
//!   refs      <┘
//!   ...engine state
//! ```

use crate::engine::ApoEngine;
use sf_core::control::ControlBlock;
use sf_core::device::DeviceInfo;
use sf_win::APO_FRIENDLY_NAME;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::core::{IUnknown, Interface, GUID, HRESULT};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_NOINTERFACE,
    E_OUTOFMEMORY, E_POINTER, S_FALSE, S_OK,
};
use windows::Win32::Media::Audio::Apo::{
    APOInitBaseStruct, APOInitSystemEffects, IAudioProcessingObject,
    IAudioProcessingObjectConfiguration, IAudioProcessingObjectRT, APO_CONNECTION_DESCRIPTOR,
    APO_CONNECTION_PROPERTY, APO_FLAG_DEFAULT, APO_REG_PROPERTIES,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CoTaskMemAlloc, IClassFactory};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

/// CLSID of the APO. Must match `sf_win::APO_CLSID`.
pub const APO_CLSID_GUID: GUID = GUID::from_u128(0x7E1F4C2A_9B3D_4A86_9F2C_5D8E6A1B7C43);

static INSTANCES: AtomicUsize = AtomicUsize::new(0);

/// State shared with the configuration watcher thread.
struct Shared {
    engine: Mutex<ApoEngine>,
    config_path: PathBuf,
    device: DeviceInfo,
    running: AtomicUsize,
}

fn open_control() -> Option<ControlBlock> {
    ControlBlock::open().ok()
}

// ---------------------------------------------------------------------------
// Vtable layouts
// ---------------------------------------------------------------------------

#[repr(C)]
struct ApoVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    reset: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    get_latency: unsafe extern "system" fn(*mut c_void, *mut i64) -> HRESULT,
    get_registration_properties:
        unsafe extern "system" fn(*mut c_void, *mut *mut APO_REG_PROPERTIES) -> HRESULT,
    initialize: unsafe extern "system" fn(*mut c_void, u32, *const u8) -> HRESULT,
    is_input_format_supported: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut *mut c_void,
    ) -> HRESULT,
    is_output_format_supported: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut *mut c_void,
    ) -> HRESULT,
    get_input_channel_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
}

#[repr(C)]
struct ConfigVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    lock_for_process: unsafe extern "system" fn(
        *mut c_void,
        u32,
        *const *const APO_CONNECTION_DESCRIPTOR,
        u32,
        *const *const APO_CONNECTION_DESCRIPTOR,
    ) -> HRESULT,
    unlock_for_process: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

#[repr(C)]
struct RtVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    apo_process: unsafe extern "system" fn(
        *mut c_void,
        u32,
        *const *const APO_CONNECTION_PROPERTY,
        u32,
        *mut *mut APO_CONNECTION_PROPERTY,
    ),
    calc_input_frames: unsafe extern "system" fn(*mut c_void, u32) -> u32,
    calc_output_frames: unsafe extern "system" fn(*mut c_void, u32) -> u32,
}

#[repr(C)]
struct ApoObject {
    vtbl: *const ApoVtbl,
    _self: *mut ApoObject,
    cfg_vtbl: *const ConfigVtbl,
    cfg_self: *mut ApoObject,
    rt_vtbl: *const RtVtbl,
    rt_self: *mut ApoObject,
    refs: AtomicU32,
    channels: AtomicUsize,
    sample_rate: AtomicU32,
    latency: AtomicU64,
    shared: Arc<Shared>,
}

impl ApoObject {
    fn new(config_path: PathBuf, device: DeviceInfo) -> Self {
        let shared = Arc::new(Shared {
            engine: Mutex::new(ApoEngine::new(
                config_path.clone(),
                device.clone(),
                48000,
                2,
                open_control(),
            )),
            config_path,
            device,
            running: AtomicUsize::new(1),
        });
        spawn_reload_thread(&shared);
        Self {
            vtbl: &APO_VTBL,
            _self: std::ptr::null_mut(),
            cfg_vtbl: &CONFIG_VTBL,
            cfg_self: std::ptr::null_mut(),
            rt_vtbl: &RT_VTBL,
            rt_self: std::ptr::null_mut(),
            refs: AtomicU32::new(1),
            channels: AtomicUsize::new(0),
            sample_rate: AtomicU32::new(48000),
            latency: AtomicU64::new(0),
            shared,
        }
    }
}

impl Drop for ApoObject {
    fn drop(&mut self) {
        self.shared.running.store(0, Ordering::Relaxed);
        INSTANCES.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Recovers the owning object from an interface pointer (the word after the
/// vtable pointer always holds the back reference).
unsafe fn owner(interface: *mut c_void) -> &'static mut ApoObject {
    // Every interface subobject stores its owning object right after the
    // vtable pointer.
    let slot = (interface as *mut u8).add(std::mem::size_of::<usize>()) as *const *mut ApoObject;
    &mut **slot
}

fn spawn_reload_thread(shared: &Arc<Shared>) {
    let weak = Arc::downgrade(shared);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(750));
        let Some(shared) = weak.upgrade() else { return };
        if shared.running.load(Ordering::Relaxed) == 0 {
            return;
        }
        let Ok(mut engine) = shared.engine.lock() else {
            return;
        };
        engine.sync_control();
        if engine.config_changed_on_disk() {
            engine.invalidate();
        }
        engine.reload();
    });
}

fn write_wide(dest: &mut [u16], text: &str) {
    let encoded: Vec<u16> = text.encode_utf16().collect();
    let count = encoded.len().min(dest.len().saturating_sub(1));
    dest[..count].copy_from_slice(&encoded[..count]);
    if count < dest.len() {
        dest[count] = 0;
    }
}

// ---------------------------------------------------------------------------
// Interface methods
// ---------------------------------------------------------------------------

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    let obj = owner(this);
    let iid = *iid;
    let result: *mut c_void = if iid == IUnknown::IID || iid == IAudioProcessingObject::IID {
        obj as *mut ApoObject as *mut c_void
    } else if iid == IAudioProcessingObjectConfiguration::IID {
        &mut obj.cfg_vtbl as *mut *const ConfigVtbl as *mut c_void
    } else if iid == IAudioProcessingObjectRT::IID {
        &mut obj.rt_vtbl as *mut *const RtVtbl as *mut c_void
    } else {
        *ppv = std::ptr::null_mut();
        return E_NOINTERFACE;
    };
    *ppv = result;
    obj.refs.fetch_add(1, Ordering::SeqCst);
    S_OK
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    owner(this).refs.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let obj = owner(this);
    let previous = obj.refs.fetch_sub(1, Ordering::SeqCst);
    if previous == 1 {
        // Last reference: the Box owns the object and drops it here.
        drop(Box::from_raw(obj as *mut ApoObject));
        0
    } else {
        previous - 1
    }
}

unsafe extern "system" fn apo_reset(this: *mut c_void) -> HRESULT {
    let obj = owner(this);
    if let Ok(mut engine) = obj.shared.engine.lock() {
        engine.reset_state();
    }
    S_OK
}

unsafe extern "system" fn apo_get_latency(this: *mut c_void, latency: *mut i64) -> HRESULT {
    if latency.is_null() {
        return E_POINTER;
    }
    *latency = owner(this).latency.load(Ordering::Relaxed) as i64;
    S_OK
}

unsafe extern "system" fn apo_get_registration_properties(
    this: *mut c_void,
    props_out: *mut *mut APO_REG_PROPERTIES,
) -> HRESULT {
    if props_out.is_null() {
        return E_POINTER;
    }
    let size = std::mem::size_of::<APO_REG_PROPERTIES>();
    let memory = CoTaskMemAlloc(size) as *mut APO_REG_PROPERTIES;
    if memory.is_null() {
        return E_OUTOFMEMORY;
    }
    std::ptr::write_bytes(memory as *mut u8, 0, size);
    let props = &mut *memory;
    props.clsid = APO_CLSID_GUID;
    props.Flags = APO_FLAG_DEFAULT;
    write_wide(&mut props.szFriendlyName, APO_FRIENDLY_NAME);
    write_wide(
        &mut props.szCopyrightInfo,
        "SonicForge contributors, GPL-2.0-or-later",
    );
    props.u32MajorVersion = 1;
    props.u32MinorVersion = 0;
    props.u32MinInputConnections = 1;
    props.u32MaxInputConnections = 1;
    props.u32MinOutputConnections = 1;
    props.u32MaxOutputConnections = 1;
    props.u32MaxInstances = u32::MAX;
    props.u32NumAPOInterfaces = 1;
    props.iidAPOInterfaceList[0] = IAudioProcessingObject::IID;
    *props_out = memory;
    let _ = this;
    S_OK
}

unsafe extern "system" fn apo_initialize(
    this: *mut c_void,
    cb_size: u32,
    data: *const u8,
) -> HRESULT {
    let obj = owner(this);
    let mut device = obj.shared.device.clone();
    if !data.is_null() && cb_size >= std::mem::size_of::<APOInitBaseStruct>() as u32 {
        // The endpoint property store carries the friendly device name, which
        // is what `Device:` sections in the configuration match against.
        if cb_size >= std::mem::size_of::<APOInitSystemEffects>() as u32 {
            let init = &*(data as *const APOInitSystemEffects);
            if let Some(store) = init.pAPOEndpointProperties.as_ref() {
                if let Ok(value) = store
                    .GetValue(&windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName)
                {
                    if let Ok(text) = PropVariantToStringAlloc(&value) {
                        if let Ok(name) = text.to_string() {
                            if !name.is_empty() {
                                device.name = name;
                            }
                        }
                    }
                }
            }
        }
    }
    let rate = obj.sample_rate.load(Ordering::Relaxed).max(8000);
    let channels = obj.channels.load(Ordering::Relaxed).max(1);
    if let Ok(mut engine) = obj.shared.engine.lock() {
        *engine = ApoEngine::new(
            obj.shared.config_path.clone(),
            device,
            rate,
            channels,
            open_control(),
        );
    }
    S_OK
}

unsafe extern "system" fn is_format_supported(
    _this: *mut c_void,
    _opposite: *mut c_void,
    _requested: *mut c_void,
    supported: *mut *mut c_void,
) -> HRESULT {
    // Returning S_OK with a null result means "the requested format is
    // acceptable unchanged", which is exactly what this APO wants.
    if !supported.is_null() {
        *supported = std::ptr::null_mut();
    }
    S_OK
}

unsafe extern "system" fn get_input_channel_count(this: *mut c_void, count: *mut u32) -> HRESULT {
    if count.is_null() {
        return E_POINTER;
    }
    *count = owner(this).channels.load(Ordering::Relaxed) as u32;
    S_OK
}

unsafe extern "system" fn lock_for_process(
    this: *mut c_void,
    num_in: u32,
    inputs: *const *const APO_CONNECTION_DESCRIPTOR,
    num_out: u32,
    outputs: *const *const APO_CONNECTION_DESCRIPTOR,
) -> HRESULT {
    if num_in == 0 || num_out == 0 || inputs.is_null() || outputs.is_null() {
        return E_INVALIDARG;
    }
    let obj = owner(this);
    let input = *inputs;
    let output = *outputs;
    if input.is_null() || output.is_null() {
        return E_INVALIDARG;
    }
    let in_format = match (*input).pFormat.as_ref() {
        Some(f) => f,
        None => return E_INVALIDARG,
    };
    let out_format = match (*output).pFormat.as_ref() {
        Some(f) => f,
        None => return E_INVALIDARG,
    };
    let in_wf = in_format.GetAudioFormat();
    let out_wf = out_format.GetAudioFormat();
    if in_wf.is_null() || out_wf.is_null() {
        return E_INVALIDARG;
    }
    let channels = (*in_wf).nChannels as usize;
    let rate = (*in_wf).nSamplesPerSec;
    if channels == 0 || rate == 0 {
        return E_INVALIDARG;
    }
    if (*in_wf).nChannels != (*out_wf).nChannels
        || (*in_wf).nSamplesPerSec != (*out_wf).nSamplesPerSec
    {
        return E_INVALIDARG;
    }
    // SonicForge processes the 32-bit float streams used by the shared-mode
    // audio engine. Integer streams are reported as unsupported instead of
    // producing garbage.
    if (*in_wf).wBitsPerSample != 32 {
        return windows::Win32::Foundation::E_NOTIMPL;
    }
    obj.channels.store(channels, Ordering::Relaxed);
    obj.sample_rate.store(rate, Ordering::Relaxed);
    if let Ok(mut engine) = obj.shared.engine.lock() {
        *engine = ApoEngine::new(
            obj.shared.config_path.clone(),
            obj.shared.device.clone(),
            rate,
            channels,
            open_control(),
        );
        let samples = engine.latency_samples() as u64;
        let latency_100ns = samples.saturating_mul(10_000_000) / rate.max(1) as u64;
        obj.latency.store(latency_100ns, Ordering::Relaxed);
    }
    let _ = (num_in, num_out);
    S_OK
}

unsafe extern "system" fn unlock_for_process(_this: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn apo_process(
    this: *mut c_void,
    num_in: u32,
    inputs: *const *const APO_CONNECTION_PROPERTY,
    num_out: u32,
    outputs: *mut *mut APO_CONNECTION_PROPERTY,
) {
    if num_in == 0 || num_out == 0 || inputs.is_null() || outputs.is_null() {
        return;
    }
    let obj = owner(this);
    let input = *inputs;
    let output = *outputs;
    if input.is_null() || output.is_null() {
        return;
    }
    let in_prop = &*input;
    let out_prop = &*output;
    let channels = obj.channels.load(Ordering::Relaxed);
    let frames = in_prop.u32ValidFrameCount.min(out_prop.u32ValidFrameCount) as usize;
    if channels == 0 || frames == 0 || in_prop.pBuffer == 0 || out_prop.pBuffer == 0 {
        return;
    }
    let samples = frames.saturating_mul(channels);
    // APO_BUFFER_FLAGS: 0 valid, 1 silent, 2 invalid.
    match in_prop.u32BufferFlags.0 {
        2 => return,
        1 => {
            let dst = std::slice::from_raw_parts_mut(out_prop.pBuffer as *mut f32, samples);
            dst.fill(0.0);
            return;
        }
        _ => {}
    }
    let src = std::slice::from_raw_parts(in_prop.pBuffer as *const f32, samples);
    let dst = std::slice::from_raw_parts_mut(out_prop.pBuffer as *mut f32, samples);
    dst.copy_from_slice(src);
    // The audio thread never blocks: a contended lock (configuration reload in
    // flight) leaves the copied audio untouched for one block.
    if let Ok(mut engine) = obj.shared.engine.try_lock() {
        engine.sync_control();
        if !engine.is_bypassed() {
            engine.process_interleaved(dst, frames);
        }
    }
}

unsafe extern "system" fn calc_input_frames(_this: *mut c_void, output_frames: u32) -> u32 {
    output_frames
}

unsafe extern "system" fn calc_output_frames(_this: *mut c_void, input_frames: u32) -> u32 {
    input_frames
}

// Subobject dispatch: recover the owner, then call the method through the
// shared implementation.
unsafe extern "system" fn cfg_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    query_interface(owner(this) as *mut ApoObject as *mut c_void, iid, ppv)
}

unsafe extern "system" fn cfg_add_ref(this: *mut c_void) -> u32 {
    add_ref(owner(this) as *mut ApoObject as *mut c_void)
}

unsafe extern "system" fn cfg_release(this: *mut c_void) -> u32 {
    release(owner(this) as *mut ApoObject as *mut c_void)
}

unsafe extern "system" fn cfg_lock_for_process(
    this: *mut c_void,
    num_in: u32,
    inputs: *const *const APO_CONNECTION_DESCRIPTOR,
    num_out: u32,
    outputs: *const *const APO_CONNECTION_DESCRIPTOR,
) -> HRESULT {
    lock_for_process(
        owner(this) as *mut ApoObject as *mut c_void,
        num_in,
        inputs,
        num_out,
        outputs,
    )
}

unsafe extern "system" fn cfg_unlock_for_process(this: *mut c_void) -> HRESULT {
    unlock_for_process(owner(this) as *mut ApoObject as *mut c_void)
}

unsafe extern "system" fn rt_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    query_interface(owner(this) as *mut ApoObject as *mut c_void, iid, ppv)
}

unsafe extern "system" fn rt_add_ref(this: *mut c_void) -> u32 {
    add_ref(owner(this) as *mut ApoObject as *mut c_void)
}

unsafe extern "system" fn rt_release(this: *mut c_void) -> u32 {
    release(owner(this) as *mut ApoObject as *mut c_void)
}

unsafe extern "system" fn rt_apo_process(
    this: *mut c_void,
    num_in: u32,
    inputs: *const *const APO_CONNECTION_PROPERTY,
    num_out: u32,
    outputs: *mut *mut APO_CONNECTION_PROPERTY,
) {
    apo_process(
        owner(this) as *mut ApoObject as *mut c_void,
        num_in,
        inputs,
        num_out,
        outputs,
    )
}

static APO_VTBL: ApoVtbl = ApoVtbl {
    query_interface,
    add_ref,
    release,
    reset: apo_reset,
    get_latency: apo_get_latency,
    get_registration_properties: apo_get_registration_properties,
    initialize: apo_initialize,
    is_input_format_supported: is_format_supported,
    is_output_format_supported: is_format_supported,
    get_input_channel_count,
};

static CONFIG_VTBL: ConfigVtbl = ConfigVtbl {
    query_interface: cfg_query_interface,
    add_ref: cfg_add_ref,
    release: cfg_release,
    lock_for_process: cfg_lock_for_process,
    unlock_for_process: cfg_unlock_for_process,
};

static RT_VTBL: RtVtbl = RtVtbl {
    query_interface: rt_query_interface,
    add_ref: rt_add_ref,
    release: rt_release,
    apo_process: rt_apo_process,
    calc_input_frames,
    calc_output_frames,
};

// ---------------------------------------------------------------------------
// Class factory
// ---------------------------------------------------------------------------

#[repr(C)]
struct FactoryVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    create_instance: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const GUID,
        *mut *mut c_void,
    ) -> HRESULT,
    lock_server: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

unsafe extern "system" fn factory_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    if *iid == IUnknown::IID || *iid == IClassFactory::IID {
        *ppv = this;
        S_OK
    } else {
        *ppv = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn factory_add_ref(_this: *mut c_void) -> u32 {
    2
}

unsafe extern "system" fn factory_release(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "system" fn factory_lock_server(_this: *mut c_void, _lock: i32) -> HRESULT {
    S_OK
}

unsafe extern "system" fn factory_create_instance(
    _this: *mut c_void,
    outer: *mut c_void,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    if riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    // Initialise the per-user configuration layout so that the first run has a
    // valid (empty) configuration instead of silently doing nothing.
    let _ = sf_win::paths::ensure_config_layout();
    let object = Box::into_raw(Box::new(ApoObject::new(
        sf_win::paths::config_file(),
        DeviceInfo::default(),
    )));
    (*object)._self = object;
    (*object).cfg_self = object;
    (*object).rt_self = object;
    INSTANCES.fetch_add(1, Ordering::SeqCst);
    let hr = query_interface(object as *mut c_void, riid, ppv);
    // Drop the constructor reference; the caller owns the returned one.
    release(object as *mut c_void);
    hr
}

static FACTORY_VTBL: FactoryVtbl = FactoryVtbl {
    query_interface: factory_query_interface,
    add_ref: factory_add_ref,
    release: factory_release,
    create_instance: factory_create_instance,
    lock_server: factory_lock_server,
};

/// The class factory object. Its first word is the vtable pointer, which is
/// what a COM interface pointer must point at.
#[repr(C)]
struct Factory {
    vtbl: *const FactoryVtbl,
}

unsafe impl Sync for Factory {}

static FACTORY: Factory = Factory {
    vtbl: &FACTORY_VTBL,
};

// ---------------------------------------------------------------------------
// DLL exports
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    unsafe {
        if *rclsid != APO_CLSID_GUID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory = &FACTORY as *const Factory as *mut c_void;
        (FACTORY_VTBL.query_interface)(factory, riid, ppv)
    }
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if INSTANCES.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

fn module_path() -> Option<PathBuf> {
    let mut buffer = vec![0u16; 2048];
    let len = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
    if len == 0 {
        return None;
    }
    buffer.truncate(len);
    Some(PathBuf::from(String::from_utf16_lossy(&buffer)))
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    let Some(path) = module_path() else {
        return E_FAIL;
    };
    match sf_win::apo::register_server(&path) {
        Ok(()) => S_OK,
        Err(e) => {
            sf_win::logging::error(&format!("DllRegisterServer failed: {e}"));
            E_FAIL
        }
    }
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    match sf_win::apo::unregister_server() {
        Ok(()) => S_OK,
        Err(e) => {
            sf_win::logging::error(&format!("DllUnregisterServer failed: {e}"));
            E_FAIL
        }
    }
}
