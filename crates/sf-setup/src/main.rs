//! SonicForge installer and uninstaller.
//!
//! A deliberately small, auditable installer: it copies the application files,
//! registers the APO class, writes the uninstall entry, and enables processing
//! on the endpoints the user chooses. It never touches another product's APO
//! registration and restores the previous effect chain on uninstall.

#[cfg(windows)]
mod windows_impl {
    use sf_win::endpoints::{self, Flow};
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\SonicForge";
    const APP_KEY: &str = r"SOFTWARE\SonicForge";

    fn install_dir() -> PathBuf {
        std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
            .join("SonicForge")
    }

    fn source_dir() -> Result<PathBuf, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        exe.parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| "cannot determine installer directory".to_string())
    }

    fn usage() {
        println!(
            "SonicForge setup {}\n\n\
             Usage:\n  \
             sf-setup install [--dir <path>] [--no-enable]\n  \
             sf-setup uninstall [--dir <path>]\n  \
             sf-setup status\n\n\
             The installer expects SonicForgeAPO.dll, sonicforge.exe,\n\
             sonicforge-editor.exe and sonicforge-tray.exe next to itself.",
            sf_core::VERSION
        );
    }

    fn arg_value(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    }

    fn install(args: &[String]) -> Result<ExitCode, String> {
        let target = arg_value(args, "--dir")
            .map(PathBuf::from)
            .unwrap_or_else(install_dir);
        let enable = !args.iter().any(|a| a == "--no-enable");
        let source = source_dir()?;
        std::fs::create_dir_all(&target)
            .map_err(|e| format!("cannot create '{}': {e}", target.display()))?;

        // Cargo names a CDylib after its library target (`sonicforge_apo`), while
        // the audio engine and the registry entry expect `SonicForgeAPO.dll`.
        // The installer is where that shipping name is pinned.
        let apo_source = ["SonicForgeAPO.dll", "sonicforge_apo.dll"]
            .iter()
            .map(|name| source.join(name))
            .find(|path| path.exists())
            .ok_or_else(|| {
                format!(
                    "neither SonicForgeAPO.dll nor sonicforge_apo.dll is in '{}'; build the workspace first",
                    source.display()
                )
            })?;
        let apo_target = target.join("SonicForgeAPO.dll");
        std::fs::copy(&apo_source, &apo_target)
            .map_err(|e| format!("cannot copy '{}': {e}", apo_target.display()))?;

        for name in [
            "sonicforge.exe",
            "sonicforge-editor.exe",
            "sonicforge-tray.exe",
        ] {
            let from = source.join(name);
            if !from.exists() {
                println!("skipping missing {name}");
                continue;
            }
            let to = target.join(name);
            std::fs::copy(&from, &to)
                .map_err(|e| format!("cannot copy '{}': {e}", to.display()))?;
        }

        let dll = apo_target;
        sf_win::apo::register_server(&dll)?;
        write_uninstall_entry(&target)?;

        // Create the configuration layout for the installing user.
        let config = sf_win::paths::ensure_config_layout().map_err(|e| e.to_string())?;
        println!("Configuration: {}", config.display());

        if enable {
            match endpoints::list(Flow::Render) {
                Ok(list) => {
                    let mut enabled = 0;
                    for endpoint in &list {
                        match sf_win::apo::enable_endpoint(endpoint) {
                            Ok(()) => {
                                println!("enabled: {}", endpoint.name);
                                enabled += 1;
                            }
                            Err(e) => println!("could not enable {}: {e}", endpoint.name),
                        }
                    }
                    if enabled == 0 {
                        println!(
                            "No endpoint was enabled. Run `sonicforge apo enable --all` from an\n\
                             elevated prompt after the installer finishes."
                        );
                    }
                }
                Err(e) => println!("could not enumerate endpoints: {e}"),
            }
        }

        println!("\nInstalled to {}", target.display());
        println!(
            "Start the editor: {}",
            target.join("sonicforge-editor.exe").display()
        );
        println!("Restart playback (or the Windows Audio service) so the effect is loaded.");
        Ok(ExitCode::SUCCESS)
    }

    fn uninstall(args: &[String]) -> Result<ExitCode, String> {
        let target = arg_value(args, "--dir")
            .map(PathBuf::from)
            .unwrap_or_else(install_dir);
        if let Ok(list) = endpoints::list(Flow::Render) {
            for endpoint in &list {
                if let Err(e) = sf_win::apo::disable_endpoint(endpoint) {
                    println!("could not restore {}: {e}", endpoint.name);
                }
            }
        }
        if let Ok(list) = endpoints::list(Flow::Capture) {
            for endpoint in &list {
                let _ = sf_win::apo::disable_endpoint(endpoint);
            }
        }
        sf_win::apo::unregister_server()?;
        remove_registry_key(UNINSTALL_KEY);
        remove_registry_key(APP_KEY);

        for name in [
            "SonicForgeAPO.dll",
            "sonicforge_apo.dll",
            "sonicforge.exe",
            "sonicforge-editor.exe",
            "sonicforge-tray.exe",
        ] {
            let path = target.join(name);
            if path.exists() {
                if let Err(e) = std::fs::remove_file(&path) {
                    println!("could not remove '{}': {e}", path.display());
                }
            }
        }
        let _ = std::fs::remove_dir(&target);
        println!(
            "SonicForge removed. Your configuration and presets were kept in {}",
            sf_win::paths::config_dir().display()
        );
        Ok(ExitCode::SUCCESS)
    }

    fn write_uninstall_entry(target: &Path) -> Result<(), String> {
        let uninstaller = target.join("sonicforge.exe");
        let entry = [
            ("DisplayName", "SonicForge".to_string()),
            ("DisplayVersion", sf_core::VERSION.to_string()),
            ("Publisher", "SonicForge contributors".to_string()),
            ("InstallLocation", target.display().to_string()),
            ("UninstallString", "sf-setup.exe uninstall".to_string()),
            ("NoModify", "1".to_string()),
        ];
        let _ = uninstaller;
        write_values(UNINSTALL_KEY, &entry)?;
        write_values(APP_KEY, &[("InstallPath", target.display().to_string())])
    }

    fn write_values(path: &str, values: &[(&str, String)]) -> Result<(), String> {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE,
            REG_OPTION_NON_VOLATILE, REG_SZ,
        };
        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }
        let sub = wide(path);
        let mut handle = HKEY::default();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(sub.as_ptr()),
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut handle,
                None,
            )
        };
        if status.0 != 0 {
            return Err(format!("cannot create HKLM\\{path} (error {})", status.0));
        }
        for (name, value) in values {
            let key = wide(name);
            let text = wide(value);
            let bytes: Vec<u8> = text.iter().flat_map(|u| u.to_le_bytes()).collect();
            let status =
                unsafe { RegSetValueExW(handle, PCWSTR(key.as_ptr()), 0, REG_SZ, Some(&bytes)) };
            if status.0 != 0 {
                unsafe {
                    let _ = RegCloseKey(handle);
                }
                return Err(format!("cannot write {name} (error {})", status.0));
            }
        }
        unsafe {
            let _ = RegCloseKey(handle);
        }
        Ok(())
    }

    fn remove_registry_key(path: &str) {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{RegDeleteTreeW, HKEY_LOCAL_MACHINE};
        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }
        let sub = wide(path);
        unsafe {
            let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(sub.as_ptr()));
        }
    }

    pub fn main() -> ExitCode {
        sf_win::logging::init();
        let args: Vec<String> = std::env::args().collect();
        let command = args.get(1).map(|s| s.as_str()).unwrap_or("help");
        let result = match command {
            "install" => install(&args),
            "uninstall" => uninstall(&args),
            "status" => {
                let list = sf_win::apo::status(Flow::Render).unwrap_or_default();
                for status in list {
                    println!(
                        "{:<12} {}",
                        if status.enabled { "enabled" } else { "-" },
                        status.endpoint.name
                    );
                }
                Ok(ExitCode::SUCCESS)
            }
            _ => {
                usage();
                Ok(ExitCode::SUCCESS)
            }
        };
        match result {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    windows_impl::main()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("sf-setup: the installer targets Windows");
}
