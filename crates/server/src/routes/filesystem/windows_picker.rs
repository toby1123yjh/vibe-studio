use std::num::NonZeroIsize;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, Win32WindowHandle, WindowHandle,
};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetDesktopWindow, GetForegroundWindow, GetShellWindow,
        GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    },
};

/// A non-owning snapshot of the foreground window that initiated a local pick.
/// Only Windows uses this: rfd passes its HWND to IFileDialog::Show, so the
/// picker stays above its owner without making it globally always-on-top.
pub(super) struct ForegroundOwner {
    hwnd: NonZeroIsize,
    process_id: u32,
}

impl ForegroundOwner {
    pub(super) fn capture() -> Option<Self> {
        // SAFETY: These APIs query OS-managed window identifiers; no memory is
        // dereferenced and no foreign window is modified or activated.
        unsafe {
            let hwnd = GetAncestor(GetForegroundWindow(), GA_ROOT);
            if hwnd.is_null()
                || hwnd == GetDesktopWindow()
                || hwnd == GetShellWindow()
                || IsWindowVisible(hwnd) == 0
                || IsIconic(hwnd) != 0
            {
                return None;
            }
            let mut process_id = 0;
            if GetWindowThreadProcessId(hwnd, &mut process_id) == 0 {
                return None;
            }
            Some(Self {
                hwnd: NonZeroIsize::new(hwnd as isize)?,
                process_id,
            })
        }
    }
}

impl HasWindowHandle for ForegroundOwner {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = self.hwnd.get() as HWND;
        // SAFETY: HWND is an opaque OS identifier, not a Rust pointer. Recheck
        // that it still names a live window in the captured process before rfd
        // copies it. rfd uses it only as the Win32 modal owner on its STA thread;
        // Windows handles owner destruction while the dialog is open.
        unsafe {
            let mut process_id = 0;
            if IsWindow(hwnd) == 0
                || GetWindowThreadProcessId(hwnd, &mut process_id) == 0
                || process_id != self.process_id
            {
                return Err(HandleError::Unavailable);
            }
            Ok(WindowHandle::borrow_raw(
                Win32WindowHandle::new(self.hwnd).into(),
            ))
        }
    }
}

impl HasDisplayHandle for ForegroundOwner {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, ptr::null_mut, time::Duration};

    use tokio::time::Instant;
    use windows_sys::{
        Win32::{
            Foundation::{HWND, LPARAM},
            UI::WindowsAndMessaging::{
                EnumWindows, GA_ROOT, GW_HWNDPREV, GW_OWNER, GetAncestor, GetForegroundWindow,
                GetWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                WM_CLOSE,
            },
        },
        core::BOOL,
    };

    struct PickerSearch<'a> {
        title: &'a str,
        hwnd: HWND,
    }

    unsafe extern "system" fn find_picker(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: EnumWindows calls synchronously with the stack-owned search.
        unsafe {
            let search = &mut *(data as *mut PickerSearch<'_>);
            let mut process_id = 0;
            GetWindowThreadProcessId(hwnd, &mut process_id);
            if process_id != std::process::id() {
                return 1;
            }
            let mut title = [0u16; 128];
            let len = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
            if String::from_utf16_lossy(&title[..len as usize]) == search.title {
                search.hwnd = hwnd;
                return 0;
            }
            1
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "opens a real Windows picker; requires an interactive foreground desktop"]
    async fn native_picker_stays_above_foreground_owner_and_cancels() {
        // Use the real foreground application in another process, just as the
        // browser and HTTP server are separate processes in an npm installation.
        // Never activate, close or destroy the external owner during this test.
        let owner = unsafe { GetAncestor(GetForegroundWindow(), GA_ROOT) };
        assert!(
            super::ForegroundOwner::capture().is_some(),
            "interactive desktop must have an eligible foreground application"
        );
        let mut owner_process_id = 0;
        unsafe { GetWindowThreadProcessId(owner, &mut owner_process_id) };
        assert_ne!(
            owner_process_id,
            std::process::id(),
            "native regression must exercise cross-process ownership"
        );

        let title = format!("Vibe Studio picker regression {}", std::process::id());
        let completed = Cell::new(false);
        let pick = async {
            let response = super::super::pick_folder(axum::Json(super::super::PickFolderRequest {
                initial_path: None,
                title: Some(title.clone()),
            }))
            .await;
            completed.set(true);
            response
        };
        let observe = async {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut search = PickerSearch {
                title: &title,
                hwnd: null_mut(),
            };
            let mut observation = None;
            loop {
                unsafe {
                    if observation.is_none() {
                        EnumWindows(Some(find_picker), &mut search as *mut _ as LPARAM);
                    }
                }
                if observation.is_none()
                    && !search.hwnd.is_null()
                    && (unsafe { IsWindowVisible(search.hwnd) != 0 } || Instant::now() >= deadline)
                {
                    let dialog = search.hwnd;
                    let actual_owner = unsafe { GetWindow(dialog, GW_OWNER) };
                    let mut above = unsafe { GetWindow(owner, GW_HWNDPREV) };
                    while !above.is_null() && above != dialog {
                        above = unsafe { GetWindow(above, GW_HWNDPREV) };
                    }
                    // Close only the picker identified by this process + test title.
                    let closed = unsafe { PostMessageW(dialog, WM_CLOSE, 0, 0) != 0 };
                    observation = Some((actual_owner, above == dialog, closed));
                }
                if completed.get() || Instant::now() >= deadline {
                    return observation;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let (response, observation) = tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(pick, observe)
        })
        .await
        .expect("native picker did not finish after test cancellation");

        let (actual_owner, above_owner, closed) =
            observation.expect("native picker was not observed");
        assert!(closed, "could not dismiss native test picker");
        assert_eq!(actual_owner, owner, "picker lost its foreground owner");
        assert!(above_owner, "picker was placed behind its owner");
        assert!(response.0.is_success());
        assert_eq!(response.0.into_data(), Some(None));
    }
}
