//! `sonicforge` — command line tools for SonicForge.
//!
//! The CLI is the escape hatch when the GUI cannot start (which matters for
//! audio software: a broken configuration must always be fixable from a
//! terminal), and the automation surface for installers and CI.

use clap::{Parser, Subcommand, ValueEnum};
use sf_core::config::{builtin_presets, Document};
use sf_core::engine::{compile_document, compile_file, CompileOptions, Stage};
use sf_core::wav::{read_wave, write_wave_f32};
use sf_win::endpoints::{self, Flow};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "sonicforge",
    version,
    about = "SonicForge — system-wide audio processing for Windows",
    long_about = "SonicForge processes all audio on an endpoint through a Rust audio engine.\n\
                  Use `sonicforge devices` to inspect endpoints, `validate` to check a\n\
                  configuration, and `apo` to install or enable the audio processing object."
)]
struct Cli {
    #[command(subcommand)]
    command: Command2,
}

#[derive(Subcommand, Debug)]
enum Command2 {
    /// List audio endpoints and their SonicForge state.
    Devices {
        /// Which endpoints to list.
        #[arg(long, value_enum, default_value = "render")]
        flow: FlowArg,
        /// Machine readable output.
        #[arg(long)]
        json: bool,
    },
    /// Validate a configuration file and print diagnostics.
    Validate {
        /// Configuration file (defaults to the installed configuration).
        file: Option<PathBuf>,
        #[arg(long, default_value_t = 48000)]
        rate: u32,
        #[arg(long, default_value_t = 2)]
        channels: usize,
    },
    /// Render a WAVE file through a configuration (offline, no audio device).
    Render {
        /// Configuration file.
        config: PathBuf,
        /// Input WAVE file.
        #[arg(long)]
        input: PathBuf,
        /// Output WAVE file.
        #[arg(long)]
        output: PathBuf,
        /// Block size in frames.
        #[arg(long, default_value_t = 480)]
        block: usize,
    },
    /// Manage the audio processing object.
    Apo {
        #[command(subcommand)]
        action: ApoAction,
    },
    /// Inspect or replace the active configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// List or apply built-in presets.
    Presets {
        #[command(subcommand)]
        action: PresetAction,
    },
    /// Collect diagnostic information.
    Diagnose {
        /// Write a diagnostic bundle into this directory.
        #[arg(long)]
        bundle: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum ApoAction {
    /// Register the COM class for the APO DLL.
    Install {
        /// Path to SonicForgeAPO.dll (defaults to the DLL next to this executable).
        #[arg(long)]
        dll: Option<PathBuf>,
    },
    /// Remove the COM class registration.
    Uninstall,
    /// Enable processing on endpoints.
    Enable {
        /// Every active endpoint of the flow.
        #[arg(long)]
        all: bool,
        /// Endpoint name, GUID or id.
        #[arg(long)]
        device: Option<String>,
        #[arg(long, value_enum, default_value = "render")]
        flow: FlowArg,
    },
    /// Disable processing and restore the previous effect chain.
    Disable {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        device: Option<String>,
        #[arg(long, value_enum, default_value = "render")]
        flow: FlowArg,
    },
    /// Show registration state per endpoint.
    Status {
        #[arg(long, value_enum, default_value = "render")]
        flow: FlowArg,
    },
    /// Load the APO DLL through COM and exercise its interfaces.
    SelfTest {
        #[arg(long)]
        dll: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Print the location of the active configuration.
    Path,
    /// Print the active configuration.
    Show,
    /// Import an Equalizer APO style configuration, keeping a backup.
    Import {
        /// Configuration file to import.
        file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum PresetAction {
    /// List the built-in presets.
    List,
    /// Write a built-in preset to the active configuration.
    Apply { name: String },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum FlowArg {
    Render,
    Capture,
}

impl From<FlowArg> for Flow {
    fn from(value: FlowArg) -> Self {
        match value {
            FlowArg::Render => Flow::Render,
            FlowArg::Capture => Flow::Capture,
        }
    }
}

fn main() -> ExitCode {
    sf_win::logging::init();
    let cli = Cli::parse();
    let result = match cli.command {
        Command2::Devices { flow, json } => cmd_devices(flow.into(), json),
        Command2::Validate {
            file,
            rate,
            channels,
        } => cmd_validate(file, rate, channels),
        Command2::Render {
            config,
            input,
            output,
            block,
        } => cmd_render(&config, &input, &output, block),
        Command2::Apo { action } => cmd_apo(action),
        Command2::Config { action } => cmd_config(action),
        Command2::Presets { action } => cmd_presets(action),
        Command2::Diagnose { bundle } => cmd_diagnose(bundle),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// devices
// ---------------------------------------------------------------------------

fn cmd_devices(flow: Flow, json: bool) -> Result<ExitCode, String> {
    let list = endpoints::list(flow)?;
    let statuses = sf_win::apo::status(flow).unwrap_or_default();
    if json {
        let payload: Vec<serde_json::Value> = list
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                    "connection": e.connection,
                    "guid": e.guid,
                    "default": e.is_default,
                    "flow": e.flow.as_str(),
                    "sonicforge": statuses.iter().find(|s| s.endpoint.guid == e.guid).map(|s| s.enabled).unwrap_or(false),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "[]".into())
        );
        return Ok(ExitCode::SUCCESS);
    }
    if list.is_empty() {
        println!("No active {} endpoints.", flow.as_str());
        return Ok(ExitCode::SUCCESS);
    }
    println!("{} endpoints ({}):", flow.as_str(), list.len());
    for endpoint in &list {
        let enabled = statuses
            .iter()
            .find(|s| s.endpoint.guid == endpoint.guid)
            .map(|s| s.enabled)
            .unwrap_or(false);
        println!(
            "  {} {} {}",
            if endpoint.is_default { "*" } else { " " },
            if enabled {
                "[SonicForge]"
            } else {
                "[          ]"
            },
            endpoint.name
        );
        println!("      guid: {}", endpoint.guid);
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// validate / render
// ---------------------------------------------------------------------------

fn default_config() -> PathBuf {
    sf_win::paths::config_file()
}

fn report_diagnostics(chain: &sf_core::engine::CompiledChain) -> bool {
    for diagnostic in &chain.diagnostics {
        println!("{diagnostic}");
    }
    if chain.diagnostics.is_empty() {
        println!("ok: no diagnostics");
    }
    !chain.has_errors()
}

fn cmd_validate(file: Option<PathBuf>, rate: u32, channels: usize) -> Result<ExitCode, String> {
    let path = file.unwrap_or_else(default_config);
    let options = CompileOptions {
        sample_rate: rate,
        channels: channels.max(1),
        stage: Stage::PostMix,
        ..Default::default()
    };
    let chain = compile_file(&path, &options)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    println!("Configuration: {}", path.display());
    println!("Stream: {} channels at {} Hz", channels, rate);
    println!(
        "Compiled {} processing stage(s); latency {} samples",
        chain.pipeline.ops().len(),
        chain.pipeline.latency_samples()
    );
    let ok = report_diagnostics(&chain);
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn cmd_render(
    config: &Path,
    input: &Path,
    output: &Path,
    block: usize,
) -> Result<ExitCode, String> {
    let audio = read_wave(input).map_err(|e| format!("cannot read '{}': {e}", input.display()))?;
    let channels = audio.channel_count().max(1);
    let frames = audio.frames();
    if frames == 0 {
        return Err("input file contains no audio".into());
    }
    let options = CompileOptions {
        sample_rate: audio.sample_rate,
        channels,
        stage: Stage::PostMix,
        ..Default::default()
    };
    // Compile from the file so `Include:` and `Convolution:` paths resolve.
    let mut chain = compile_file(config, &options)
        .map_err(|e| format!("cannot read '{}': {e}", config.display()))?;
    if chain.has_errors() {
        report_diagnostics(&chain);
        return Err("configuration has errors, refusing to render".into());
    }

    let pipeline_channels = chain.pipeline.channels().max(channels);
    let mut out = vec![vec![0.0f32; frames]; pipeline_channels];
    let block = block.clamp(32, 8192);
    let mut position = 0usize;
    while position < frames {
        let count = block.min(frames - position);
        let mut planar: Vec<Vec<f32>> = Vec::with_capacity(pipeline_channels);
        for ch in 0..pipeline_channels {
            let source = audio.channels.get(ch);
            let mut row = vec![0.0f32; count];
            if let Some(src) = source {
                row.copy_from_slice(&src[position..position + count]);
            }
            planar.push(row);
        }
        chain.pipeline.process(&mut planar);
        for (ch, row) in planar.iter().enumerate() {
            out[ch][position..position + count].copy_from_slice(row);
        }
        position += count;
    }

    // Compensate the convolver latency so the rendered file stays aligned.
    let latency = chain.pipeline.latency_samples();
    if latency > 0 && latency < frames {
        for channel in out.iter_mut() {
            channel.drain(..latency);
            channel.resize(frames, 0.0);
        }
    }

    let peak = out
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0f32, |acc, v| acc.max(v.abs()));
    let rms = {
        let sum: f64 = out
            .iter()
            .flat_map(|c| c.iter())
            .map(|v| (*v as f64) * (*v as f64))
            .sum();
        let count = (frames * pipeline_channels).max(1);
        (sum / count as f64).sqrt() as f32
    };
    write_wave_f32(output, audio.sample_rate, &out)
        .map_err(|e| format!("cannot write '{}': {e}", output.display()))?;
    println!(
        "Rendered {} frames, {} channels at {} Hz -> {}",
        frames,
        pipeline_channels,
        audio.sample_rate,
        output.display()
    );
    println!(
        "Peak {peak:.4} ({:.2} dBFS), RMS {rms:.4}",
        20.0 * peak.max(1e-9).log10()
    );
    if peak > 1.0 {
        println!("warning: output exceeds full scale; lower the preamp to avoid clipping");
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// APO management
// ---------------------------------------------------------------------------

fn resolve_dll(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        if path.exists() {
            return Ok(path);
        }
        return Err(format!("'{}' does not exist", path.display()));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Cargo names the CDylib `sonicforge_apo.dll`; installations use the
    // shipped name `SonicForgeAPO.dll`. Both are accepted here.
    let candidate = exe
        .parent()
        .map(|dir| dir.join("SonicForgeAPO.dll"))
        .filter(|path| path.exists())
        .or_else(|| exe.parent().map(|dir| dir.join("sonicforge_apo.dll")));
    match candidate {
        Some(path) if path.exists() => Ok(path),
        Some(path) => Err(format!(
            "cannot find '{}'; pass --dll explicitly",
            path.display()
        )),
        None => Err("cannot determine executable directory".into()),
    }
}

fn cmd_apo(action: ApoAction) -> Result<ExitCode, String> {
    match action {
        ApoAction::Install { dll } => {
            let path = resolve_dll(dll)?;
            sf_win::apo::register_server(&path)?;
            println!("Registered {} as {}", path.display(), sf_win::APO_CLSID);
            println!("Next: sonicforge apo enable --all");
            Ok(ExitCode::SUCCESS)
        }
        ApoAction::Uninstall => {
            sf_win::apo::unregister_server()?;
            println!("Removed the SonicForge COM registration.");
            println!("Endpoints that still reference it: sonicforge apo status");
            Ok(ExitCode::SUCCESS)
        }
        ApoAction::Enable { all, device, flow } => {
            let flow: Flow = flow.into();
            let targets = select_endpoints(flow, all, device)?;
            if targets.is_empty() {
                return Err("no matching endpoints".into());
            }
            let missing = targets.iter().find(|e| !e.id.is_empty()).is_some();
            if !missing {
                return Err("no usable endpoints found".into());
            }
            for endpoint in &targets {
                match sf_win::apo::enable_endpoint(endpoint) {
                    Ok(()) => println!("enabled: {}", endpoint.name),
                    Err(e) => return Err(format!("{}: {e}", endpoint.name)),
                }
            }
            println!("\nRestart the affected audio stream (or the Windows Audio service) so that the effect is picked up.");
            Ok(ExitCode::SUCCESS)
        }
        ApoAction::Disable { all, device, flow } => {
            let flow: Flow = flow.into();
            let targets = select_endpoints(flow, all, device)?;
            for endpoint in &targets {
                match sf_win::apo::disable_endpoint(endpoint) {
                    Ok(()) => println!("disabled: {}", endpoint.name),
                    Err(e) => eprintln!("{}: {e}", endpoint.name),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        ApoAction::Status { flow } => {
            let flow: Flow = flow.into();
            let statuses = sf_win::apo::status(flow)?;
            if statuses.is_empty() {
                println!("No active {} endpoints.", flow.as_str());
                return Ok(ExitCode::SUCCESS);
            }
            for status in statuses {
                let state = if status.enabled {
                    "enabled"
                } else {
                    "not enabled"
                };
                println!("{state:>11}  {}", status.endpoint.name);
                if let Some(clsid) = &status.effect_clsid {
                    println!("             effect CLSID: {clsid}");
                }
                if status.has_backup {
                    println!("             previous chain backed up (restored on disable)");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        ApoAction::SelfTest { dll } => {
            let path = resolve_dll(dll)?;
            #[cfg(windows)]
            {
                self_test(&path)
            }
            #[cfg(not(windows))]
            {
                let _ = path;
                Err("the COM self test requires Windows".into())
            }
        }
    }
}

fn select_endpoints(
    flow: Flow,
    all: bool,
    device: Option<String>,
) -> Result<Vec<endpoints::Endpoint>, String> {
    if let Some(selector) = device {
        let endpoint = endpoints::find(flow, &selector)?
            .ok_or_else(|| format!("no endpoint matches '{selector}'"))?;
        return Ok(vec![endpoint]);
    }
    if all {
        return endpoints::list(flow);
    }
    Err("pass --all or --device <name>".into())
}

#[cfg(windows)]
fn self_test(dll_path: &Path) -> Result<ExitCode, String> {
    use std::ffi::c_void;
    use windows::core::{IUnknown, Interface, GUID, HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::Apo::{
        IAudioProcessingObject, IAudioProcessingObjectConfiguration, APO_REG_PROPERTIES,
    };
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

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
    #[repr(C)]
    struct ComVtbl {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }
    #[repr(C)]
    struct ApoVtblProbe {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        reset: usize,
        get_latency: unsafe extern "system" fn(*mut c_void, *mut i64) -> HRESULT,
        get_registration_properties:
            unsafe extern "system" fn(*mut c_void, *mut *mut APO_REG_PROPERTIES) -> HRESULT,
    }

    let wide: Vec<u16> = dll_path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let module = LoadLibraryW(PCWSTR(wide.as_ptr()))
            .map_err(|e| format!("cannot load '{}': {e}", dll_path.display()))?;
        let proc = GetProcAddress(module, windows::core::s!("DllGetClassObject"))
            .ok_or("DLL does not export DllGetClassObject")?;
        let get_class_object: unsafe extern "system" fn(
            *const GUID,
            *const GUID,
            *mut *mut c_void,
        ) -> HRESULT = std::mem::transmute(proc);

        let mut factory: *mut c_void = std::ptr::null_mut();
        let hr = get_class_object(
            &apo_clsid(),
            &windows::Win32::System::Com::IClassFactory::IID,
            &mut factory,
        );
        if hr.is_err() {
            return Err(format!("DllGetClassObject failed: {hr:?}"));
        }
        println!("DllGetClassObject: ok");

        let vtbl = *(factory as *const *const FactoryVtbl);
        let mut apo: *mut c_void = std::ptr::null_mut();
        let hr = ((*vtbl).create_instance)(
            factory,
            std::ptr::null_mut(),
            &IAudioProcessingObject::IID,
            &mut apo,
        );
        ((*vtbl).release)(factory);
        if hr.is_err() {
            return Err(format!("CreateInstance failed: {hr:?}"));
        }
        println!("IClassFactory::CreateInstance(IAudioProcessingObject): ok");

        let apo_vtbl = *(apo as *const *const ApoVtblProbe);
        let mut latency: i64 = -1;
        let hr = ((*apo_vtbl).get_latency)(apo, &mut latency);
        if hr.is_err() {
            return Err(format!("GetLatency failed: {hr:?}"));
        }
        println!("GetLatency: {latency} (100 ns units)");

        let mut props: *mut APO_REG_PROPERTIES = std::ptr::null_mut();
        let hr = ((*apo_vtbl).get_registration_properties)(apo, &mut props);
        if hr.is_err() || props.is_null() {
            return Err(format!("GetRegistrationProperties failed: {hr:?}"));
        }
        let props_ref = &*props;
        let name_len = props_ref
            .szFriendlyName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(props_ref.szFriendlyName.len());
        let name = String::from_utf16_lossy(&props_ref.szFriendlyName[..name_len]);
        println!(
            "Registration: '{name}' v{}.{}",
            props_ref.u32MajorVersion, props_ref.u32MinorVersion
        );
        windows::Win32::System::Com::CoTaskMemFree(Some(props as *const c_void));

        // QueryInterface for the other interfaces proves the vtable layout.
        let mut config: *mut c_void = std::ptr::null_mut();
        let hr = ((*apo_vtbl).query_interface)(
            apo,
            &IAudioProcessingObjectConfiguration::IID,
            &mut config,
        );
        if hr.is_err() {
            return Err(format!("QueryInterface(configuration) failed: {hr:?}"));
        }
        println!("QueryInterface(IAudioProcessingObjectConfiguration): ok");
        let cfg_vtbl = *(config as *const *const ComVtbl);
        ((*cfg_vtbl).release)(config);

        ((*apo_vtbl).release)(apo);
        // Prove the object is gone.
        let unload = GetProcAddress(module, windows::core::s!("DllCanUnloadNow"))
            .ok_or("missing DllCanUnloadNow")?;
        let can_unload: unsafe extern "system" fn() -> HRESULT = std::mem::transmute(unload);
        let hr = can_unload();
        println!(
            "DllCanUnloadNow after release: {}",
            if hr == windows::Win32::Foundation::S_OK {
                "S_OK"
            } else {
                "S_FALSE (still in use)"
            }
        );
        let _ = IUnknown::IID;
        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(windows)]
fn apo_clsid() -> windows::core::GUID {
    // Parsed from the shared string so there is one source of truth.
    sf_win::apo::clsid_guid()
}

// ---------------------------------------------------------------------------
// configuration & presets
// ---------------------------------------------------------------------------

fn backup_if_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        let backup = path.with_extension("txt.bak");
        std::fs::copy(path, &backup)
            .map_err(|e| format!("cannot back up '{}': {e}", path.display()))?;
    }
    Ok(())
}

fn cmd_config(action: ConfigAction) -> Result<ExitCode, String> {
    match action {
        ConfigAction::Path => {
            println!("{}", default_config().display());
            Ok(ExitCode::SUCCESS)
        }
        ConfigAction::Show => {
            let path = default_config();
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    println!("# {}", path.display());
                    print!("{text}");
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => Err(format!("cannot read '{}': {e}", path.display())),
            }
        }
        ConfigAction::Import { file } => {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| format!("cannot read '{}': {e}", file.display()))?;
            let doc = Document::parse(&text);
            if doc.lines.is_empty() {
                return Err("the file does not contain any recognised commands".into());
            }
            // Validate before touching the live configuration.
            let options = CompileOptions {
                sample_rate: 48000,
                channels: 2,
                ..Default::default()
            };
            let chain = compile_document(&doc, &options);
            for diagnostic in &chain.diagnostics {
                println!("{diagnostic}");
            }
            if chain.has_errors() {
                return Err("import rejected: the configuration has errors".into());
            }
            let target = sf_win::paths::ensure_config_layout().map_err(|e| e.to_string())?;
            backup_if_exists(&target)?;
            std::fs::copy(&file, &target)
                .map_err(|e| format!("cannot write '{}': {e}", target.display()))?;
            println!(
                "Imported {} command(s) into {}",
                doc.lines.len(),
                target.display()
            );
            println!("The APO reloads the configuration automatically.");
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn cmd_presets(action: PresetAction) -> Result<ExitCode, String> {
    match action {
        PresetAction::List => {
            for preset in builtin_presets() {
                println!("{:28} {}", preset.name, preset.description);
            }
            Ok(ExitCode::SUCCESS)
        }
        PresetAction::Apply { name } => {
            let preset = builtin_presets()
                .into_iter()
                .find(|p| p.name.eq_ignore_ascii_case(&name))
                .ok_or_else(|| {
                    format!("unknown preset '{name}' (try `sonicforge presets list`)")
                })?;
            let target = sf_win::paths::ensure_config_layout().map_err(|e| e.to_string())?;
            backup_if_exists(&target)?;
            std::fs::write(&target, preset.to_config_text())
                .map_err(|e| format!("cannot write '{}': {e}", target.display()))?;
            println!("Applied '{}' to {}", preset.name, target.display());
            Ok(ExitCode::SUCCESS)
        }
    }
}

// ---------------------------------------------------------------------------
// diagnose
// ---------------------------------------------------------------------------

fn cmd_diagnose(bundle: Option<PathBuf>) -> Result<ExitCode, String> {
    let mut report = String::new();
    report.push_str(&format!("SonicForge {}\n", sf_core::VERSION));
    report.push_str(&format!(
        "OS: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    report.push_str(&format!("Configuration: {}\n", default_config().display()));
    report.push_str(&format!("Logs: {}\n\n", sf_win::paths::log_dir().display()));

    let config_path = default_config();
    let config_text = std::fs::read_to_string(&config_path).unwrap_or_default();
    let doc = Document::parse(&config_text);
    let options = CompileOptions {
        sample_rate: 48000,
        channels: 2,
        ..Default::default()
    };
    let chain = compile_document(&doc, &options);
    report.push_str(&format!(
        "Configuration: {} commands, {} stages, latency {} samples\n",
        doc.lines.len(),
        chain.pipeline.ops().len(),
        chain.pipeline.latency_samples()
    ));
    for diagnostic in &chain.diagnostics {
        report.push_str(&format!("  {diagnostic}\n"));
    }

    report.push_str("\nEndpoints:\n");
    let statuses = sf_win::apo::status(Flow::Render).unwrap_or_default();
    for status in &statuses {
        report.push_str(&format!(
            "  {:<12} {} (effect: {})\n",
            if status.enabled { "enabled" } else { "-" },
            status.endpoint.name,
            status.effect_clsid.as_deref().unwrap_or("none")
        ));
    }
    if statuses.is_empty() {
        report.push_str("  (no render endpoints or no permission to read them)\n");
    }

    report.push_str("\nRecent log:\n");
    for line in sf_win::logging::tail(60) {
        report.push_str(&format!("  {line}\n"));
    }

    print!("{report}");

    if let Some(dir) = bundle {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create '{}': {e}", dir.display()))?;
        std::fs::write(dir.join("diagnostics.txt"), &report).map_err(|e| e.to_string())?;
        if !config_text.is_empty() {
            std::fs::write(dir.join("config.txt"), &config_text).map_err(|e| e.to_string())?;
        }
        println!("\nBundle written to {}", dir.display());
    }
    Ok(ExitCode::SUCCESS)
}
