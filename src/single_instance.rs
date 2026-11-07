//! A named-mutex guard so two copies of DeskOrg cannot both listen for the
//! same desktop gestures (which would cancel each other's toggles).

#[cfg(windows)]
mod imp {
    use std::ptr::null;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;

    /// `wait_for_previous` is used by the elevated relaunch: the new process
    /// may start before the old one has finished exiting, so retry briefly
    /// instead of immediately focusing the old instance and quitting.
    pub fn acquire(wait_for_previous: bool) -> bool {
        let name = wide("Local\\DeskOrg.SingleInstance");
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            // A null security descriptor means the default for the current user.
            unsafe { SetLastError(0) };
            let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
            if handle.is_null() {
                // If the OS refuses to create the mutex, do not block the app.
                return true;
            }
            if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
                // Keep this handle for the process lifetime.
                return true;
            }
            // Do not keep a handle to the existing mutex: that would keep the
            // kernel object alive after the old process exits.
            unsafe { CloseHandle(handle) };
            if !wait_for_previous || Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn acquire(_wait_for_previous: bool) -> bool {
        true
    }
}

pub use imp::acquire;
