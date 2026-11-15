//! SonicForge tray application.
//!
//! A small always-running helper that gives quick access to the essentials:
//! open the editor, toggle bypass, reload the configuration, and open the log
//! folder. It talks to the APO through the shared control block, so toggling
//! bypass takes effect within one audio block.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    if let Err(e) = windows_impl::run() {
        sf_win::logging::error(&format!("tray failed: {e}"));
        eprintln!("sonicforge-tray: {e}");
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("sonicforge-tray: the tray application requires Windows");
}

#[cfg(windows)]
mod windows_impl {
    use sf_core::control::ControlBlock;
    use std::path::PathBuf;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
        DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, MessageBoxW, PostQuitMessage,
        RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, CS_HREDRAW,
        CS_VREDRAW, IDI_APPLICATION, MB_ICONINFORMATION, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG,
        TPM_LEFTBUTTON, TPM_RETURNCMD, WM_APP, WM_DESTROY, WM_LBUTTONDBLCLK, WM_RBUTTONUP,
        WNDCLASSW, WS_OVERLAPPED,
    };

    const TRAY_ID: u32 = 1;
    const CALLBACK_MESSAGE: u32 = WM_APP + 1;
    const MENU_OPEN: usize = 1;
    const MENU_BYPASS: usize = 2;
    const MENU_RELOAD: usize = 3;
    const MENU_LOGS: usize = 4;
    const MENU_EXIT: usize = 5;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn sibling_binary(name: &str) -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let candidate = exe.parent()?.join(name);
        candidate.exists().then_some(candidate)
    }

    fn open_editor() {
        if let Some(editor) = sibling_binary("sonicforge-editor.exe") {
            let _ = std::process::Command::new(editor).spawn();
        } else {
            notify("Cannot find sonicforge-editor.exe next to the tray application.");
        }
    }

    fn notify(message: &str) {
        let text = wide(message);
        let title = wide("SonicForge");
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_ICONINFORMATION,
            );
        }
    }

    fn toggle_bypass() {
        match ControlBlock::open() {
            Ok(control) => {
                let snap = control.read();
                let updated = control.set_bypass(!snap.bypass);
                sf_win::logging::info(&format!("bypass set to {}", updated.bypass));
            }
            Err(_) => notify(
                "The SonicForge control block is not available. Start the editor or reinstall.",
            ),
        }
    }

    fn reload_config() {
        match ControlBlock::open() {
            Ok(control) => {
                let snap = control.read();
                // A bump asks the APO watcher to recompile the configuration.
                let mut next = snap;
                next.config_generation = snap.config_generation.wrapping_add(1);
                control.write(next);
                sf_win::logging::info("configuration reload requested from tray");
            }
            Err(_) => notify("The SonicForge control block is not available."),
        }
    }

    fn show_menu(hwnd: HWND) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let bypassed = ControlBlock::open()
                .map(|c| c.read().bypass)
                .unwrap_or(false);
            let open = wide("Open SonicForge editor");
            let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, PCWSTR(open.as_ptr()));
            let bypass = wide("Bypass processing");
            let flags = if bypassed {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(menu, flags, MENU_BYPASS, PCWSTR(bypass.as_ptr()));
            let reload = wide("Reload configuration");
            let _ = AppendMenuW(menu, MF_STRING, MENU_RELOAD, PCWSTR(reload.as_ptr()));
            let logs = wide("Open log folder");
            let _ = AppendMenuW(menu, MF_STRING, MENU_LOGS, PCWSTR(logs.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let exit = wide("Exit");
            let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, PCWSTR(exit.as_ptr()));

            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let _ = SetForegroundWindow(hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_LEFTBUTTON | TPM_RETURNCMD,
                point.x,
                point.y,
                0,
                hwnd,
                None,
            )
            .0 as usize;
            let _ = DestroyMenu(menu);

            match command {
                MENU_OPEN => open_editor(),
                MENU_BYPASS => toggle_bypass(),
                MENU_RELOAD => reload_config(),
                MENU_LOGS => {
                    let _ = std::process::Command::new("explorer.exe")
                        .arg(sf_win::paths::log_dir())
                        .spawn();
                }
                MENU_EXIT => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            CALLBACK_MESSAGE => {
                // lParam carries the mouse message for tray icons.
                let mouse = lparam.0 as u32;
                if mouse == WM_RBUTTONUP {
                    show_menu(hwnd);
                } else if mouse == WM_LBUTTONDBLCLK {
                    open_editor();
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    fn add_icon(hwnd: HWND) -> bool {
        unsafe {
            let mut data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ID,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: CALLBACK_MESSAGE,
                hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
                ..Default::default()
            };
            let tip = wide("SonicForge audio processing");
            let count = tip.len().min(data.szTip.len());
            data.szTip[..count].copy_from_slice(&tip[..count]);
            Shell_NotifyIconW(NIM_ADD, &data).as_bool()
        }
    }

    fn remove_icon(hwnd: HWND) {
        unsafe {
            let data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ID,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }

    pub fn run() -> Result<(), String> {
        sf_win::logging::init();
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class_name = wide("SonicForgeTrayWindow");
            let window_title = wide("SonicForge");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err("RegisterClassW failed".into());
            }
            let hinst: HINSTANCE = instance.into();
            let hwnd = CreateWindowExW(
                Default::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(window_title.as_ptr()),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                hinst,
                None,
            )
            .map_err(|e| format!("CreateWindowExW failed: {e}"))?;
            if !add_icon(hwnd) {
                return Err("Shell_NotifyIconW(NIM_ADD) failed".into());
            }
            sf_win::logging::info("tray application started");

            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, None, 0, 0);
                if result.0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            remove_icon(hwnd);
        }
        sf_win::logging::info("tray application stopped");
        Ok(())
    }
}
