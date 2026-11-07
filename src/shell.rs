//! Small, dependency-light Windows shell integration:
//!
//! * show / hide desktop icons through the shell's own "Show desktop icons" command;
//! * detect a double-click on empty desktop space with a 20 ms polling thread;
//! * a global Ctrl+Alt+D hotkey poller;
//! * wallpaper and rounded dark title bar.
//!
//! No administrator rights and no Explorer injection are involved.

#[derive(Clone, Copy, Debug)]
pub enum ShellEvent {
    /// Request toggling the desktop icons / fence layer.
    Toggle,
    /// Request showing/hiding the fence layer from the tray menu.
    ToggleFences,
    /// Show and focus the main control window.
    ShowMain,
    /// Quit the application even when close-to-background is enabled.
    Exit,
}

#[cfg(windows)]
mod imp {
    use super::ShellEvent;
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CAPTION_COLOR,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, SetBkMode,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE,
        REG_DWORD, REG_SZ,
    };
    use windows_sys::Win32::System::Threading::Sleep;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetDoubleClickTime, ReleaseCapture, VK_CONTROL, VK_MENU,
    };
    use windows_sys::Win32::UI::Shell::{
        IsUserAnAdmin, SHGetFileInfoW, ShellExecuteW, Shell_NotifyIconGetRect, Shell_NotifyIconW,
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD, NIM_DELETE,
        NIM_MODIFY, NOTIFYICONDATAW, NOTIFYICONDATAW_0, NOTIFYICONIDENTIFIER, SHFILEINFOW,
        SHGFI_ICON, SHGFI_LARGEICON,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
        DestroyWindow, DispatchMessageW, DrawIconEx, EnumWindows, FindWindowExW, FindWindowW,
        GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetMessageW, GetParent,
        GetSystemMetrics, GetWindowLongPtrW, GetWindowTextW, IsIconic, IsWindow, IsWindowVisible,
        LoadIconW, PostMessageW, PostQuitMessage, RegisterClassW,
        RegisterWindowMessageW, SendMessageTimeoutW, SendMessageW, SetForegroundWindow, SetTimer,
        SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW, TrackPopupMenu,
        TranslateMessage, WindowFromPoint, DI_NORMAL, GA_ROOT, GWL_EXSTYLE, HTCAPTION,
        HWND_BOTTOM, HWND_TOPMOST, IDI_APPLICATION, MF_SEPARATOR, MF_STRING, MSG,
        SMTO_ABORTIFHUNG,
        SM_CXDOUBLECLK, SM_CXVIRTUALSCREEN, SM_CYDOUBLECLK, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE, SW_SHOW,
        SW_SHOWNOACTIVATE, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE,
        WM_COMMAND,
        WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_NCLBUTTONDOWN, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_EX_APPWINDOW,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_GETDESKWALLPAPER, SPI_SETDESKWALLPAPER,
    };

    const DESKTOP_TOGGLE_COMMAND: WPARAM = 0x7402;
    const LVM_GETNEXTITEM: u32 = 0x1000 + 12;
    const LVNI_SELECTED: LPARAM = 0x0002;
    const VK_D: i32 = 0x44;
    const VK_LBUTTON: i32 = 0x01;
    const CAPTION_DARK: u32 = 0x0012_171A;
    const CAPTION_LIGHT: u32 = 0x00F5F7FC;

    static EVENT_TX: OnceLock<Mutex<Sender<ShellEvent>>> = OnceLock::new();
    static WAKE_CTX: OnceLock<eframe::egui::Context> = OnceLock::new();
    static WATCHERS_STARTED: OnceLock<()> = OnceLock::new();
    static DOUBLE_CLICK_ENABLED: AtomicBool = AtomicBool::new(true);
    static ICONS_HIDDEN_BY_APP: AtomicBool = AtomicBool::new(false);

    pub fn start_watchers(
        tx: Sender<ShellEvent>,
        ctx: eframe::egui::Context,
    ) -> Result<(), String> {
        if WATCHERS_STARTED.set(()).is_err() {
            return Ok(());
        }
        let _ = EVENT_TX.set(Mutex::new(tx));
        let _ = WAKE_CTX.set(ctx);

        std::thread::Builder::new()
            .name("deskorg-input".to_owned())
            .spawn(input_watcher_thread)
            .map(|_| ())
            .map_err(|error| format!("无法启动桌面手势监听线程：{error}"))
    }

    pub fn set_double_click_enabled(enabled: bool) {
        DOUBLE_CLICK_ENABLED.store(enabled, Ordering::Relaxed);
    }

    // ── Notification-area icon ──────────────────────────────────────────────
    const TRAY_CALLBACK: u32 = WM_APP + 1;
    const TRAY_SHOW_MAIN: u32 = WM_APP + 2;
    static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
    static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
    static TRAY_PROMOTE_STARTED: AtomicBool = AtomicBool::new(false);
    static TRAY_LEFT_DOWN: AtomicBool = AtomicBool::new(false);

    fn emit(event: ShellEvent) {
        if let Some(tx) = EVENT_TX.get() {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(event);
            }
        }
        // 后台线程发来的事件需要在 UI 线程下一次重绘时被处理；显式唤醒，
        // 这样主循环可以降到 ~1 Hz 空闲轮询而热键仍然即时响应。
        if let Some(ctx) = WAKE_CTX.get() {
            ctx.request_repaint();
        }
    }

    pub fn start_tray() -> Result<(), String> {
        if TRAY_HWND.load(Ordering::SeqCst) != 0 {
            return Ok(());
        }
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        // The tray owns a dedicated message loop. egui stops pumping window
        // messages when its root viewport is hidden, so the tray cannot live on
        // the UI thread if it must stay clickable in the background.
        std::thread::Builder::new()
            .name("deskorg-tray".to_owned())
            .spawn(move || unsafe {
                let instance = GetModuleHandleW(null());
                let class_name = wide("DeskOrgTrayWindow");
                let window_title = wide("DeskOrg");
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(tray_wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: instance,
                    hIcon: null_mut(),
                    hCursor: null_mut(),
                    hbrBackground: null_mut(),
                    lpszMenuName: null(),
                    lpszClassName: class_name.as_ptr(),
                };
                let _class_atom = RegisterClassW(&wc);
                #[cfg(debug_assertions)]
                eprintln!(
                    "tray: RegisterClassW atom={_class_atom} err={:?}",
                    std::io::Error::last_os_error()
                );
                let hwnd = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class_name.as_ptr(),
                    window_title.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    null_mut(),
                    null_mut(),
                    instance,
                    null(),
                );
                #[cfg(debug_assertions)]
                eprintln!(
                    "tray: CreateWindowExW hwnd={hwnd:?} is_window={} err={:?}",
                    IsWindow(hwnd),
                    std::io::Error::last_os_error()
                );
                if hwnd.is_null() {
                    let _ = ready_tx.send(Err("无法创建托盘窗口".to_owned()));
                    return;
                }
                ShowWindow(hwnd, SW_HIDE);
                TRAY_HWND.store(hwnd as isize, Ordering::SeqCst);
                let added = tray_add(hwnd);
                schedule_tray_promotion();
                SetTimer(hwnd, 1usize, 40, None);
                let _ = ready_tx.send(if added {
                    Ok(())
                } else {
                    Err("无法添加托盘图标".to_owned())
                });

                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                TRAY_HWND.store(0, Ordering::SeqCst);
            })
            .map_err(|error| format!("无法启动托盘线程：{error}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(result) => result,
            Err(_) => Err("托盘初始化超时".to_owned()),
        }
    }

    pub fn stop_tray() {
        let hwnd = TRAY_HWND.load(Ordering::SeqCst) as HWND;
        if !hwnd.is_null() {
            unsafe {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }
    }

    pub fn tray_alive() -> bool {
        TRAY_HWND.load(Ordering::SeqCst) != 0
    }

    #[allow(clippy::manual_dangling_ptr)]
    fn tray_add(hwnd: HWND) -> bool {
        unsafe {
            let instance = GetModuleHandleW(null());
            // Resource id 1 is the RT_GROUP_ICON written by tools/set_icon.py.
            let _resource_icon = LoadIconW(instance, 1 as *const u16);
            let mut icon = _resource_icon;
            if icon.is_null() {
                icon = LoadIconW(null_mut(), IDI_APPLICATION);
            }
            let mut data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: TRAY_CALLBACK,
                hIcon: icon,
                ..Default::default()
            };
            let tip: Vec<u16> = "DeskOrg 桌面整理"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let len = tip.len().min(data.szTip.len());
            data.szTip[..len].copy_from_slice(&tip[..len]);
            if IsWindow(hwnd) == 0 {
                return false;
            }
            let result = Shell_NotifyIconW(NIM_ADD, &data);
            #[cfg(debug_assertions)]
            eprintln!(
                "tray: NIM_ADD hwnd={hwnd:?} is_window={} resource_icon={_resource_icon:?} \
                 final_icon={icon:?} cbSize={} flags={} result={result} err={:?}",
                IsWindow(hwnd),
                data.cbSize,
                data.uFlags,
                std::io::Error::last_os_error()
            );
            result != 0
        }
    }

    /// Windows 11 puts new notification icons in the overflow flyout by
    /// default. Promote this executable's icon in the per-user
    /// `NotifyIconSettings` store so it sits directly in the tray.
    fn promote_tray_icon() -> bool {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let target = exe.to_string_lossy().to_ascii_lowercase();
        let subkey = wide(r"Control Panel\NotifyIconSettings");
        let value_path = wide("ExecutablePath");
        let value_uid = wide("UID");
        let value_promoted = wide("IsPromoted");
        let mut root: HKEY = null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_READ | KEY_SET_VALUE,
                &mut root,
            )
        } != 0
        {
            return false;
        }

        let mut index = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut name_len = name.len() as u32;
            let status = unsafe {
                RegEnumKeyExW(
                    root,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            if status == 259 {
                // ERROR_NO_MORE_ITEMS
                break;
            }
            index += 1;
            if status != 0 {
                continue;
            }
            let key_name = wide(&String::from_utf16_lossy(&name[..name_len as usize]));
            let mut key: HKEY = null_mut();
            if unsafe {
                RegOpenKeyExW(
                    root,
                    key_name.as_ptr(),
                    0,
                    KEY_QUERY_VALUE | KEY_SET_VALUE,
                    &mut key,
                )
            } != 0
            {
                continue;
            }

            let mut path_buffer = [0u16; 1024];
            let mut path_bytes = (path_buffer.len() * 2) as u32;
            let mut path_type = 0u32;
            let query = unsafe {
                RegQueryValueExW(
                    key,
                    value_path.as_ptr(),
                    null(),
                    &mut path_type,
                    path_buffer.as_mut_ptr() as *mut u8,
                    &mut path_bytes,
                )
            };
            let path = if query == 0 {
                let count = (path_bytes as usize / 2).min(path_buffer.len());
                let end = path_buffer[..count]
                    .iter()
                    .position(|ch| *ch == 0)
                    .unwrap_or(count);
                String::from_utf16_lossy(&path_buffer[..end])
            } else {
                String::new()
            };

            let mut uid = 1u32;
            let mut uid_bytes = 4u32;
            let mut uid_type = 0u32;
            let _ = unsafe {
                RegQueryValueExW(
                    key,
                    value_uid.as_ptr(),
                    null(),
                    &mut uid_type,
                    &mut uid as *mut u32 as *mut u8,
                    &mut uid_bytes,
                )
            };

            if path.eq_ignore_ascii_case(&target) && uid == 1 {
                let promoted = 1u32;
                let written = unsafe {
                    RegSetValueExW(
                        key,
                        value_promoted.as_ptr(),
                        0,
                        REG_DWORD,
                        &promoted as *const u32 as *const u8,
                        4,
                    )
                };
                unsafe { RegCloseKey(key) };
                unsafe { RegCloseKey(root) };
                return written == 0;
            }
            unsafe { RegCloseKey(key) };
        }
        unsafe { RegCloseKey(root) };
        false
    }

    fn schedule_tray_promotion() {
        if TRAY_PROMOTE_STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::Builder::new()
            .name("deskorg-tray-promote".to_owned())
            .spawn(|| {
                for _ in 0..24 {
                    if promote_tray_icon() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .ok();
    }

    fn tray_remove(hwnd: HWND) {
        unsafe {
            let data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }

    fn taskbar_created_message() -> u32 {
        *TASKBAR_CREATED
            .get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) })
    }

    fn poll_tray_left_click(hwnd: HWND) {
        unsafe {
            let down = GetAsyncKeyState(VK_LBUTTON) < 0;
            let was_down = TRAY_LEFT_DOWN.swap(down, Ordering::SeqCst);
            // 只在按下的那一帧查询托盘图标位置，避免空闲时每 40 ms
            // 调一次 Shell_NotifyIconGetRect。
            if !down || was_down {
                return;
            }
            let mut point = POINT::default();
            if GetCursorPos(&mut point) == 0 {
                return;
            }
            let identifier = NOTIFYICONIDENTIFIER {
                cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
                hWnd: hwnd,
                uID: 1,
                ..Default::default()
            };
            let mut rect = RECT::default();
            if Shell_NotifyIconGetRect(&identifier, &mut rect) != 0 {
                return;
            }
            let inside = point.x >= rect.left
                && point.x < rect.right
                && point.y >= rect.top
                && point.y < rect.bottom;
            if inside {
                show_main_window();
                emit(ShellEvent::ShowMain);
            }
        }
    }

    fn show_tray_menu(hwnd: HWND) {
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            let open = wide("打开 DeskOrg");
            let toggle = wide("显示 / 隐藏框框");
            let exit = wide("退出 DeskOrg");
            AppendMenuW(menu, MF_STRING, 1, open.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, 2, toggle.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, 3, exit.as_ptr());

            let mut point = POINT::default();
            GetCursorPos(&mut point);
            SetForegroundWindow(hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                hwnd,
                null(),
            );
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            match command {
                1 => {
                    show_main_window();
                    emit(ShellEvent::ShowMain);
                }
                2 => emit(ShellEvent::ToggleFences),
                3 => emit(ShellEvent::Exit),
                _ => {}
            }
        }
    }

    unsafe extern "system" fn tray_wndproc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == taskbar_created_message() {
            let _ = tray_add(hwnd);
            schedule_tray_promotion();
            return 0;
        }
        if message == WM_TIMER {
            poll_tray_left_click(hwnd);
            return 0;
        }
        if message == TRAY_SHOW_MAIN {
            show_main_window();
            emit(ShellEvent::ShowMain);
            return 0;
        }
        if message == TRAY_CALLBACK {
            match (lparam as u32) & 0xFFFF {
                WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    show_main_window();
                    emit(ShellEvent::ShowMain);
                    return 0;
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    show_tray_menu(hwnd);
                    return 0;
                }
                _ => {}
            }
        }
        if message == WM_CLOSE {
            DestroyWindow(hwnd);
            return 0;
        }
        if message == WM_DESTROY {
            tray_remove(hwnd);
            PostQuitMessage(0);
            return 0;
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }

    pub fn desktop_icons_visible() -> Option<bool> {
        let list = find_desktop_listview()?;
        Some(unsafe { IsWindowVisible(list) != 0 })
    }

    pub fn set_desktop_icons_visible(show: bool) -> Result<bool, String> {
        let list = find_desktop_listview()
            .ok_or_else(|| "找不到桌面图标窗口，请确认 Windows 资源管理器正在运行".to_owned())?;
        let current = unsafe { IsWindowVisible(list) != 0 };
        if current == show {
            return Ok(current);
        }

        let defview = unsafe { GetParent(list) };
        if defview.is_null() {
            return Err("找不到桌面视图窗口".to_owned());
        }

        // shell 的“显示桌面图标”命令是同步处理的。用带超时的 SendMessage
        // 代替 PostMessage + 主线程 Sleep 轮询，Explorer 卡死时也不会把
        // DeskOrg 的 UI 一起冻结近一秒。
        let mut sent_result: usize = 0;
        let _sent = unsafe {
            SendMessageTimeoutW(
                defview,
                WM_COMMAND,
                DESKTOP_TOGGLE_COMMAND,
                0,
                SMTO_ABORTIFHUNG,
                200,
                &mut sent_result,
            )
        };

        let visible = || unsafe { IsWindowVisible(list) != 0 };
        if visible() == show {
            ICONS_HIDDEN_BY_APP.store(!show, Ordering::SeqCst);
            return Ok(show);
        }

        // Fallback for shells that ignore the toggle command. This changes the
        // list view's visibility for the current Explorer session only.
        let _fallback = unsafe { ShowWindow(list, if show { SW_SHOW } else { SW_HIDE }) };
        if visible() == show {
            ICONS_HIDDEN_BY_APP.store(!show, Ordering::SeqCst);
            return Ok(show);
        }

        #[cfg(debug_assertions)]
        eprintln!(
            "desktop toggle failed: list={list:?} defview={defview:?} \
             sent={_sent} visible={} fallback={_fallback}",
            visible()
        );
        Err("Windows 没有响应桌面图标切换命令，请稍后再试".to_owned())
    }

    /// 主窗口收进托盘时，用系统通知气泡显示重要提示。
    pub fn show_tray_balloon(title: &str, text: &str, is_error: bool) {
        let hwnd = TRAY_HWND.load(Ordering::SeqCst) as HWND;
        if hwnd.is_null() {
            return;
        }
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_INFO,
            dwInfoFlags: if is_error { NIIF_ERROR } else { NIIF_INFO },
            Anonymous: NOTIFYICONDATAW_0 { uTimeout: 4000 },
            ..Default::default()
        };
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, text);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    fn copy_wide(destination: &mut [u16], text: &str) {
        let encoded: Vec<u16> = text
            .encode_utf16()
            .take(destination.len().saturating_sub(1))
            .collect();
        destination[..encoded.len()].copy_from_slice(&encoded);
    }

    /// 程序 panic 时尽量把 DeskOrg 自己隐藏的桌面图标恢复，避免用户只剩下
    /// 一张空桌面。由 `main` 安装的 panic hook 调用。
    pub fn restore_icons_after_panic() {
        if ICONS_HIDDEN_BY_APP.load(Ordering::SeqCst) {
            let _ = set_desktop_icons_visible(true);
        }
    }

    /// 整个虚拟桌面的屏幕坐标（含多显示器），用于毛玻璃 UV 归一化。
    pub fn virtual_desktop_rect() -> Option<(f32, f32, f32, f32)> {
        let (x, y, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        (width > 0 && height > 0).then_some((
            x as f32,
            y as f32,
            width as f32,
            height as f32,
        ))
    }

    pub fn current_wallpaper() -> Option<PathBuf> {
        let mut buffer = [0u16; 260];
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETDESKWALLPAPER,
                buffer.len() as u32,
                buffer.as_mut_ptr() as *mut c_void,
                0,
            )
        };
        if ok == 0 {
            return None;
        }
        let length = buffer
            .iter()
            .position(|ch| *ch == 0)
            .unwrap_or(buffer.len());
        if length == 0 {
            return None;
        }
        let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length]));
        path.is_file().then_some(path)
    }

    /// Extract the real 32x32 shell icon for a file or folder.
    pub fn file_icon_rgba(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
        const SIZE: i32 = 32;
        let path_wide = wide(&path.to_string_lossy());
        let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let result = unsafe {
            SHGetFileInfoW(
                path_wide.as_ptr(),
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            )
        };
        if result == 0 || info.hIcon.is_null() {
            return Err("无法读取文件图标".to_owned());
        }

        let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE,
            // Negative height means a top-down DIB.
            biHeight: -SIZE,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: (SIZE * SIZE * 4) as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };

        let mut pixels: *mut c_void = null_mut();
        let dc = unsafe { CreateCompatibleDC(null_mut()) };
        if dc.is_null() {
            unsafe { DestroyIcon(info.hIcon) };
            return Err("无法创建图标绘图上下文".to_owned());
        }
        let bitmap =
            unsafe { CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut pixels, null_mut(), 0) };
        if bitmap.is_null() || pixels.is_null() {
            unsafe {
                DeleteDC(dc);
                DestroyIcon(info.hIcon);
            }
            return Err("无法创建图标位图".to_owned());
        }

        let old = unsafe { SelectObject(dc, bitmap) };
        unsafe {
            SetBkMode(dc, 1); // TRANSPARENT
            DrawIconEx(dc, 0, 0, info.hIcon, SIZE, SIZE, 0, null_mut(), DI_NORMAL);
            SelectObject(dc, old);
        }

        let len = (SIZE * SIZE * 4) as usize;
        let raw = unsafe { std::slice::from_raw_parts(pixels as *const u8, len) };
        let mut rgba = vec![0u8; len];
        let mut has_alpha = false;
        let raw_pixels = raw.as_chunks::<4>().0;
        let rgba_pixels = rgba.as_chunks_mut::<4>().0;
        for (src, dst) in raw_pixels.iter().zip(rgba_pixels.iter_mut()) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
            has_alpha |= src[3] != 0;
        }
        if !has_alpha {
            // Very old 24-bit icons have no alpha channel. Keep the shape but
            // make the background transparent instead of drawing a black tile.
            for (src, pixel) in raw_pixels.iter().zip(rgba_pixels.iter_mut()) {
                let b = src[0];
                let g = src[1];
                let r = src[2];
                if r == 0 && g == 0 && b == 0 {
                    pixel[3] = 0;
                } else {
                    pixel[3] = 255;
                }
            }
        }

        unsafe {
            DeleteObject(bitmap);
            DeleteDC(dc);
            DestroyIcon(info.hIcon);
        }
        Ok((rgba, SIZE as u32, SIZE as u32))
    }

    pub fn set_wallpaper(path: &Path) -> Result<(), String> {
        if !path.is_file() {
            return Err("壁纸文件不存在".to_owned());
        }
        let wide_path = wide(&path.to_string_lossy());
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_SETDESKWALLPAPER,
                0,
                wide_path.as_ptr() as *mut c_void,
                SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
            )
        };
        if ok == 0 {
            Err(format!("设置壁纸失败：{}", std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    /// Remove a fence from the taskbar / Alt-Tab without touching its content.
    pub fn make_tool_window(title: &str) {
        let title = wide(title);
        let hwnd = unsafe { FindWindowW(null(), title.as_ptr()) };
        if hwnd.is_null() {
            return;
        }
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let ex = (ex & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    }

    /// Start a native Windows move loop for a borderless fence window.
    ///
    /// `ViewportCommand::StartDrag` is a no-op for immediate child viewports on
    /// some eframe/winit versions, so drive the standard non-client drag
    /// directly. The call blocks until the user releases the mouse button.
    pub fn start_window_drag(title: &str) {
        let title = wide(title);
        let hwnd = unsafe { FindWindowW(null(), title.as_ptr()) };
        if hwnd.is_null() {
            return;
        }
        unsafe {
            ReleaseCapture();
            SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as WPARAM, 0);
        }
    }

    /// Add or remove the current executable from the per-user Run key.
    ///
    /// Autostart launches with `--background` so logging in does not pop the
    /// control window over the desktop.
    pub fn set_auto_start(enabled: bool) -> Result<(), String> {
        const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
        const RUN_VALUE: &str = "DeskOrg";
        let subkey = wide(RUN_KEY);
        let name = wide(RUN_VALUE);
        let mut key: HKEY = null_mut();
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if opened != 0 {
            return Err(format!(
                "无法打开开机启动注册表项：{}",
                std::io::Error::from_raw_os_error(opened as i32)
            ));
        }

        let status = if enabled {
            let exe = match std::env::current_exe() {
                Ok(exe) => exe,
                Err(error) => {
                    unsafe { RegCloseKey(key) };
                    return Err(format!("无法读取程序路径：{error}"));
                }
            };
            let command = format!("\"{}\" --background", exe.display());
            let data = wide(&command);
            unsafe {
                RegSetValueExW(
                    key,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr() as *const u8,
                    (data.len() * 2) as u32,
                )
            }
        } else {
            let status = unsafe { RegDeleteValueW(key, name.as_ptr()) };
            // ERROR_FILE_NOT_FOUND means there was nothing to remove.
            if status == 2 {
                0
            } else {
                status
            }
        };
        unsafe { RegCloseKey(key) };

        if status == 0 {
            Ok(())
        } else {
            Err(format!(
                "更新开机启动失败：{}",
                std::io::Error::from_raw_os_error(status as i32)
            ))
        }
    }

    /// Bring the already-running instance (if any) to the foreground.
    pub fn hide_main_window() {
        let title = wide("DeskOrg · 桌面整理与美化");
        let hwnd = unsafe { FindWindowW(null(), title.as_ptr()) };
        if !hwnd.is_null() {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    pub fn show_main_window() {
        let title = wide("DeskOrg · 桌面整理与美化");
        let hwnd = unsafe { FindWindowW(null(), title.as_ptr()) };
        if !hwnd.is_null() {
            unsafe {
                // ShowWindow(SW_SHOW) can be ignored after winit hid the
                // viewport; SWP_SHOWWINDOW reliably makes it visible again.
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_SHOWWINDOW,
                );
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
        }
    }

    pub fn focus_existing_window() {
        let class = wide("DeskOrgTrayWindow");
        let title = wide("DeskOrg");
        let tray = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
        if !tray.is_null() {
            unsafe {
                PostMessageW(tray, TRAY_SHOW_MAIN, 0, 0);
            }
            return;
        }
        show_main_window();
    }

    pub fn is_elevated() -> bool {
        unsafe { IsUserAnAdmin() != 0 }
    }

    /// 用 UAC 重新启动当前 exe，并带上 `--elevated` 让新实例等待旧实例退出。
    pub fn relaunch_elevated() -> Result<(), String> {
        use std::os::windows::ffi::OsStrExt;

        let exe = std::env::current_exe()
            .map_err(|error| format!("无法读取程序路径：{error}"))?;
        let file: Vec<u16> = exe
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let verb = wide("runas");
        let parameters = wide("--elevated");
        let result = unsafe {
            ShellExecuteW(
                null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                parameters.as_ptr(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        if result as isize > 32 {
            Ok(())
        } else {
            Err(format!(
                "未能以管理员身份启动（错误码 {}）",
                result as isize
            ))
        }
    }

    const CONTROL_TITLE: &str = "DeskOrg · 桌面整理与美化";

    static FENCE_GUARD_ENABLED: AtomicBool = AtomicBool::new(false);
    static FENCE_GUARD_RAISED: AtomicBool = AtomicBool::new(false);
    static FENCE_GUARD_CONTROL_HIDDEN: AtomicBool = AtomicBool::new(false);
    static FENCE_GUARD_STARTED: OnceLock<()> = OnceLock::new();

    /// 框框层级守护：Win+D 会把桌面层抬到最上面，AlwaysOnBottom 的框框
    /// 会被盖住。这里独立检测“桌面/任务栏在前台且没有其他可见应用”的
    /// Show Desktop 状态，把框框临时抬到桌面之上；切回普通应用后再压回底层。
    pub fn set_fence_guard(enabled: bool) {
        FENCE_GUARD_ENABLED.store(enabled, Ordering::Relaxed);
        if !enabled {
            FENCE_GUARD_RAISED.store(false, Ordering::Relaxed);
            return;
        }
        if FENCE_GUARD_STARTED.set(()).is_err() {
            return;
        }
        std::thread::Builder::new()
            .name("deskorg-fence-guard".to_owned())
            .spawn(fence_guard_loop)
            .ok();
    }

    fn fence_guard_loop() {
        loop {
            if FENCE_GUARD_ENABLED.load(Ordering::Relaxed) {
                fence_guard_tick();
            } else if FENCE_GUARD_CONTROL_HIDDEN.swap(false, Ordering::Relaxed) {
                // 守护关闭时如果还藏着主窗口，把它找回来。
                let title = wide(CONTROL_TITLE);
                let hwnd = unsafe { FindWindowW(null(), title.as_ptr()) };
                if !hwnd.is_null() {
                    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    struct GuardSearch {
        fences: Vec<HWND>,
        visible_apps: usize,
        control: HWND,
        control_iconic: bool,
    }

    unsafe extern "system" fn guard_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam as *mut GuardSearch);
        let class = window_class(hwnd);
        let title = window_title(hwnd);

        if title == CONTROL_TITLE {
            search.control = hwnd;
            search.control_iconic = IsIconic(hwnd) != 0;
        }

        if title.starts_with("DeskOrgFence-") {
            search.fences.push(hwnd);
            return 1;
        }
        if class.eq_ignore_ascii_case("Progman") || class.eq_ignore_ascii_case("WorkerW") {
            return 1;
        }
        if class.starts_with("Shell_")
            || class.eq_ignore_ascii_case("NotifyIconOverflowWindow")
            || class.eq_ignore_ascii_case("Windows.UI.Core.CoreWindow")
            || class.eq_ignore_ascii_case("XamlExplorerHostIslandWindow")
            || class.eq_ignore_ascii_case("TopLevelWindowForOverflowXamlIsland")
            || class.eq_ignore_ascii_case("MultitaskingViewFrame")
            || class.eq_ignore_ascii_case("ForegroundStaging")
        {
            return 1;
        }
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return 1;
        }
        // Win+D 对 UWP 等窗口使用 DWM cloaking，而不是最小化。
        if window_is_cloaked(hwnd) {
            return 1;
        }
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        // 工具窗口和置顶的桌面挂件（天气、桌面宠物等）不算“普通应用”。
        if ex_style & (WS_EX_TOOLWINDOW | WS_EX_TOPMOST) != 0 {
            return 1;
        }
        if title.is_empty() {
            return 1;
        }
        search.visible_apps += 1;
        1
    }

    fn window_class(hwnd: HWND) -> String {
        let mut buffer = [0u16; 128];
        let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        if len <= 0 {
            String::new()
        } else {
            String::from_utf16_lossy(&buffer[..len as usize])
        }
    }

    fn window_title(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        let len = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        if len <= 0 {
            String::new()
        } else {
            String::from_utf16_lossy(&buffer[..len as usize])
        }
    }

    fn window_is_cloaked(hwnd: HWND) -> bool {
        let mut cloaked: u32 = 0;
        let hr = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                14, // DWMWA_CLOAKED
                &mut cloaked as *mut u32 as *mut c_void,
                std::mem::size_of::<u32>() as u32,
            )
        };
        hr == 0 && cloaked != 0
    }

    fn fence_guard_tick() {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return;
        }
        let fg_class = window_class(foreground);
        let desktop_layer = fg_class.eq_ignore_ascii_case("Progman")
            || fg_class.eq_ignore_ascii_case("WorkerW")
            || fg_class.eq_ignore_ascii_case("Shell_TrayWnd")
            || fg_class.eq_ignore_ascii_case("Shell_SecondaryTrayWnd")
            || fg_class.eq_ignore_ascii_case("NotifyIconOverflowWindow");

        let mut search = GuardSearch {
            fences: Vec::new(),
            visible_apps: 0,
            control: null_mut(),
            control_iconic: false,
        };
        unsafe {
            EnumWindows(
                Some(guard_callback),
                &mut search as *mut GuardSearch as LPARAM,
            );
        }
        if search.fences.contains(&foreground) {
            return;
        }

        let raise = desktop_layer && search.visible_apps == 0;
        let raise_changed = FENCE_GUARD_RAISED.swap(raise, Ordering::Relaxed) != raise;
        // Win+D 的桌面层是 topmost，普通 HWND_TOP 仍会被盖住。
        let after = if raise { HWND_TOPMOST } else { HWND_BOTTOM };

        if raise {
            // Win+D 会把主窗口最小化，egui 暂停绘制让框框表面变空。
            // 改为 SW_HIDE 后 RequestPaintWhileHidden 仍然生效，框框能继续画。
            if search.control_iconic && !search.control.is_null() {
                unsafe { ShowWindow(search.control, SW_HIDE) };
                FENCE_GUARD_CONTROL_HIDDEN.store(true, Ordering::Relaxed);
            }
        } else if FENCE_GUARD_CONTROL_HIDDEN.swap(false, Ordering::Relaxed)
            && !search.control.is_null()
        {
            unsafe {
                ShowWindow(search.control, SW_SHOWNOACTIVATE);
                if IsIconic(search.control) != 0 {
                    ShowWindow(search.control, SW_RESTORE);
                }
            }
        }

        #[cfg(debug_assertions)]
        eprintln!(
            "fence_guard: fg={fg_class} visible_apps={} raise={raise}",
            search.visible_apps
        );

        for hwnd in search.fences {
            let moved = unsafe {
                SetWindowPos(
                    hwnd,
                    after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
            #[cfg(debug_assertions)]
            eprintln!(
                "fence_guard: hwnd={hwnd:?} raise={raise} moved={moved} err={:?}",
                std::io::Error::last_os_error()
            );
            let _ = moved;
        }

        if raise_changed {
            if let Some(ctx) = WAKE_CTX.get() {
                ctx.request_repaint();
            }
        }
    }

    pub fn open_in_explorer(path: &Path) -> Result<(), String> {
        std::process::Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|err| format!("无法打开资源管理器：{err}"))
    }

    /// 弹出 Windows 资源管理器同款原生右键菜单（打开方式、复制、重命名、属性等）。
    pub fn show_shell_context_menu(path: &Path) -> Result<(), String> {
        use windows::core::{HSTRING, PCSTR};
        use windows::Win32::Foundation::{POINT, RECT};
        use windows::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        use windows::Win32::UI::Shell::{
            BHID_SFUIObject, IContextMenu, IShellItem, SHCreateItemFromParsingName, CMF_NORMAL,
            CMINVOKECOMMANDINFO,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow,
            TrackPopupMenu, WindowFromPoint, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
            TPM_TOPALIGN, WM_NULL,
        };

        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let should_uninit = hr.is_ok();

            let result = (|| -> Result<(), String> {
                let name = HSTRING::from(path.as_os_str());
                let item: IShellItem = SHCreateItemFromParsingName(&name, None)
                    .map_err(|err| format!("SHCreateItemFromParsingName: {err}"))?;
                let context: IContextMenu = item
                    .BindToHandler(None, &BHID_SFUIObject)
                    .map_err(|err| format!("BindToHandler: {err}"))?;

                let menu = CreatePopupMenu().map_err(|err| format!("CreatePopupMenu: {err}"))?;

                if context
                    .QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL)
                    .is_err()
                {
                    let _ = DestroyMenu(menu);
                    return Err("QueryContextMenu 失败".to_owned());
                }

                let mut point = POINT::default();
                GetCursorPos(&mut point).map_err(|err| format!("GetCursorPos: {err}"))?;
                let hwnd = WindowFromPoint(point);
                let _ = SetForegroundWindow(hwnd);

                let command = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON,
                    point.x,
                    point.y,
                    None,
                    hwnd,
                    Option::<*const RECT>::None,
                );
                // 让菜单在点击别处时能正常关闭。
                let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());

                if command.0 != 0 {
                    let info = CMINVOKECOMMANDINFO {
                        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                        fMask: 0,
                        hwnd,
                        lpVerb: PCSTR((command.0 - 1) as usize as *const u8),
                        lpParameters: PCSTR::null(),
                        lpDirectory: PCSTR::null(),
                        nShow: 1, // SW_SHOWNORMAL
                        dwHotKey: 0,
                        hIcon: Default::default(),
                    };
                    let invoked = context.InvokeCommand(&info);
                    let _ = DestroyMenu(menu);
                    invoked.map_err(|err| format!("InvokeCommand: {err}"))?;
                } else {
                    let _ = DestroyMenu(menu);
                }
                Ok(())
            })();

            if should_uninit {
                CoUninitialize();
            }
            result
        }
    }

    pub fn set_window_theme(hwnd: HWND, dark: bool) {
        unsafe {
            let dark_value: i32 = i32::from(dark);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                &dark_value as *const i32 as *const c_void,
                std::mem::size_of::<i32>() as u32,
            );
            let corner: i32 = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &corner as *const i32 as *const c_void,
                std::mem::size_of::<i32>() as u32,
            );
            let caption = if dark { CAPTION_DARK } else { CAPTION_LIGHT };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR as u32,
                &caption as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );
        }
    }

    fn find_desktop_listview() -> Option<HWND> {
        let progman = unsafe { FindWindowW(wide("Progman").as_ptr(), null()) };
        if !progman.is_null() {
            if let Some(list) = listview_in(progman) {
                return Some(list);
            }
        }

        struct Search {
            found: HWND,
        }
        unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
            // 只接受桌面窗口。文件资源管理器窗口也有
            // SHELLDLL_DefView/SysListView32，误选它会导致图标可见性判断完全错误。
            let mut class = [0u16; 32];
            let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
            if len > 0 {
                let class = String::from_utf16_lossy(&class[..len as usize]);
                if !class.eq_ignore_ascii_case("Progman")
                    && !class.eq_ignore_ascii_case("WorkerW")
                {
                    return 1;
                }
            } else {
                return 1;
            }

            let search = &mut *(lparam as *mut Search);
            if let Some(list) = listview_in(hwnd) {
                search.found = list;
                return 0;
            }
            1
        }

        let mut search = Search { found: null_mut() };
        unsafe {
            EnumWindows(Some(callback), &mut search as *mut Search as LPARAM);
        }
        if search.found.is_null() {
            None
        } else {
            Some(search.found)
        }
    }

    fn listview_in(parent: HWND) -> Option<HWND> {
        let defview = unsafe {
            FindWindowExW(
                parent,
                null_mut(),
                wide("SHELLDLL_DefView").as_ptr(),
                null(),
            )
        };
        if defview.is_null() {
            return None;
        }
        let list =
            unsafe { FindWindowExW(defview, null_mut(), wide("SysListView32").as_ptr(), null()) };
        if list.is_null() {
            None
        } else {
            Some(list)
        }
    }

    fn is_desktop_surface(point: POINT) -> bool {
        let hwnd = unsafe { WindowFromPoint(point) };
        if hwnd.is_null() {
            return false;
        }
        let Some(list) = find_desktop_listview() else {
            return false;
        };
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        let list_root = unsafe { GetAncestor(list, GA_ROOT) };
        !root.is_null() && root == list_root
    }

    fn is_empty_desktop_click(point: POINT) -> bool {
        let hwnd = unsafe { WindowFromPoint(point) };
        if hwnd.is_null() {
            return false;
        }
        let Some(list) = find_desktop_listview() else {
            return false;
        };
        if unsafe { IsWindowVisible(list) } == 0 {
            // In zen mode there is no icon to hit, so any desktop double-click
            // should bring the icons back.
            return true;
        }

        let mut selected: usize = usize::MAX;
        let result = unsafe {
            SendMessageTimeoutW(
                list,
                LVM_GETNEXTITEM,
                usize::MAX,
                LVNI_SELECTED,
                SMTO_ABORTIFHUNG,
                40,
                &mut selected,
            )
        };
        if result == 0 {
            // The shell is busy. Do not surprise the user with a toggle.
            return false;
        }
        (selected as isize) < 0
    }

    /// One lightweight polling thread for both gestures. Polling avoids a
    /// global mouse hook (fewer antivirus false positives) and works even when
    /// the main window is minimised.
    fn input_watcher_thread() {
        let clock = Instant::now();
        let mut hotkey_was_down = false;
        let mut mouse_was_down = false;
        let mut last_click_tick = 0u64;
        let mut last_x = 0i32;
        let mut last_y = 0i32;
        let mut last_on_desktop = false;

        loop {
            let hotkey_down =
                key_down(VK_CONTROL as i32) && key_down(VK_MENU as i32) && key_down(VK_D);
            if hotkey_down && !hotkey_was_down {
                send_event(ShellEvent::Toggle);
            }
            hotkey_was_down = hotkey_down;

            let state = unsafe { GetAsyncKeyState(VK_LBUTTON) } as u16;
            let mouse_down = state & 0x8000 != 0;
            let pressed_since_last_poll = state & 0x0001 != 0;
            if (pressed_since_last_poll && !mouse_was_down) || (mouse_down && !mouse_was_down) {
                let mut point = POINT { x: 0, y: 0 };
                if unsafe { GetCursorPos(&mut point) } != 0
                    && DOUBLE_CLICK_ENABLED.load(Ordering::Relaxed)
                {
                    let now = clock.elapsed().as_millis() as u64;
                    let on_desktop = is_desktop_surface(point);
                    let double_click_time = unsafe { GetDoubleClickTime() }.max(200) as u64;
                    let max_dx = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) }.max(8);
                    let max_dy = unsafe { GetSystemMetrics(SM_CYDOUBLECLK) }.max(8);
                    let close_enough =
                        (point.x - last_x).abs() <= max_dx && (point.y - last_y).abs() <= max_dy;
                    let is_double = last_click_tick > 0
                        && now.saturating_sub(last_click_tick) <= double_click_time
                        && close_enough
                        && last_on_desktop
                        && on_desktop;

                    if is_double {
                        last_click_tick = 0;
                        if is_empty_desktop_click(point) {
                            send_event(ShellEvent::Toggle);
                        }
                    } else {
                        last_click_tick = now;
                    }
                    last_x = point.x;
                    last_y = point.y;
                    last_on_desktop = on_desktop;
                }
            } else if !DOUBLE_CLICK_ENABLED.load(Ordering::Relaxed) {
                last_click_tick = 0;
            }
            mouse_was_down = mouse_down;

            unsafe { Sleep(20) };
        }
    }

    fn key_down(vk: i32) -> bool {
        (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
    }

    fn send_event(event: ShellEvent) {
        emit(event);
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(windows)]
pub use imp::{
    current_wallpaper, desktop_icons_visible, file_icon_rgba, focus_existing_window,
    hide_main_window, make_tool_window, open_in_explorer, restore_icons_after_panic,
    is_elevated, relaunch_elevated, set_auto_start, set_desktop_icons_visible,
    set_double_click_enabled, set_fence_guard, set_wallpaper, set_window_theme,
    show_main_window, show_shell_context_menu, show_tray_balloon, start_tray, start_watchers,
    start_window_drag, stop_tray, tray_alive, virtual_desktop_rect,
};

#[cfg(not(windows))]
mod imp {
    use super::ShellEvent;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Sender;

    pub fn start_watchers(
        _tx: Sender<ShellEvent>,
        _ctx: eframe::egui::Context,
    ) -> Result<(), String> {
        Ok(())
    }
    pub fn set_double_click_enabled(_enabled: bool) {}
    pub fn desktop_icons_visible() -> Option<bool> {
        None
    }
    pub fn set_desktop_icons_visible(_show: bool) -> Result<bool, String> {
        Err("仅支持 Windows".to_owned())
    }
    pub fn set_wallpaper(_path: &Path) -> Result<(), String> {
        Err("仅支持 Windows".to_owned())
    }
    pub fn current_wallpaper() -> Option<PathBuf> {
        None
    }
    pub fn open_in_explorer(_path: &Path) -> Result<(), String> {
        Err("仅支持 Windows".to_owned())
    }
    pub fn show_shell_context_menu(_path: &Path) -> Result<(), String> {
        Err("仅支持 Windows".to_owned())
    }
    pub fn focus_existing_window() {}
    pub fn hide_main_window() {}
    pub fn show_main_window() {}
    pub fn make_tool_window(_title: &str) {}
    pub fn start_window_drag(_title: &str) {}
    pub fn start_tray() -> Result<(), String> {
        Err("仅支持 Windows".to_owned())
    }
    pub fn stop_tray() {}
    pub fn tray_alive() -> bool {
        false
    }
    pub fn virtual_desktop_rect() -> Option<(f32, f32, f32, f32)> {
        None
    }
    pub fn restore_icons_after_panic() {}
    pub fn show_tray_balloon(_title: &str, _text: &str, _is_error: bool) {}
    pub fn is_elevated() -> bool {
        false
    }
    pub fn relaunch_elevated() -> Result<(), String> {
        Err("仅支持 Windows".to_owned())
    }
    pub fn set_fence_guard(_enabled: bool) {}
    pub fn set_auto_start(_enabled: bool) -> Result<(), String> {
        Err("仅支持 Windows".to_owned())
    }
    pub fn file_icon_rgba(_path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
        Err("仅支持 Windows".to_owned())
    }
    pub fn set_window_theme(_hwnd: *mut std::ffi::c_void, _dark: bool) {}
}

#[cfg(not(windows))]
pub use imp::{
    current_wallpaper, desktop_icons_visible, file_icon_rgba, focus_existing_window,
    hide_main_window, make_tool_window, open_in_explorer, restore_icons_after_panic,
    is_elevated, relaunch_elevated, set_auto_start, set_desktop_icons_visible,
    set_double_click_enabled, set_fence_guard, set_wallpaper, set_window_theme,
    show_main_window, show_shell_context_menu, show_tray_balloon, start_tray, start_watchers,
    start_window_drag, stop_tray, tray_alive, virtual_desktop_rect,
};

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    #[ignore = "interactive desktop probe"]
    fn probe_desktop_icons() {
        let visible = desktop_icons_visible();
        println!("desktop icons visible = {visible:?}");
        assert!(visible.is_some());
    }

    #[test]
    #[ignore = "writes a probe image"]
    fn probe_file_icon() {
        let path = Path::new(r"C:\Windows\System32\notepad.exe");
        let (rgba, width, height) = file_icon_rgba(path).unwrap();
        let transparent = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] == 0)
            .count();
        println!("icon {width}x{height}, transparent pixels = {transparent}");
        image::save_buffer(
            "target/icon-probe.png",
            &rgba,
            width,
            height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}
