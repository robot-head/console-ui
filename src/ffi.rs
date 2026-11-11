//! Small, versioned C ABI for native macOS shells. JSON is a transport detail of the bridge.
//! All strings returned by `console_ui_dispatch` and `console_ui_view` are owned by the caller
//! and must be released exactly once with `console_ui_string_free`.
use crate::{
    config::Settings,
    workspace::{Action, Workspace},
};
use anyhow::{Context, Result};
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Mutex,
};

pub struct Bridge(Mutex<Workspace>);
thread_local! {static LAST_ERROR:RefCell<CString>=RefCell::new(CString::new("").unwrap());}
fn error(error: impl std::fmt::Display) {
    let text = error.to_string().replace('\0', "�");
    LAST_ERROR.with(|slot| *slot.borrow_mut() = CString::new(text).unwrap());
}
fn json(value: serde_json::Value) -> *mut c_char {
    CString::new(value.to_string()).unwrap().into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn console_ui_abi_version() -> u32 {
    1
}
/// Creates a controller using the platform's persisted settings. Null indicates failure.
#[unsafe(no_mangle)]
pub extern "C" fn console_ui_create() -> *mut Bridge {
    match catch_unwind(|| -> Result<Bridge> {
        let path = crate::preferences::Preferences::path()?;
        let prefs = crate::preferences::Preferences::load(&path)?;
        Ok(Bridge(Mutex::new(Workspace::new(
            prefs.settings,
            None,
            None,
        )?)))
    }) {
        Ok(Ok(bridge)) => Box::into_raw(Box::new(bridge)),
        Ok(Err(e)) => {
            error(format!("{e:#}"));
            std::ptr::null_mut()
        }
        Err(_) => {
            error("Native controller initialization failed");
            std::ptr::null_mut()
        }
    }
}
/// Creates a controller with an explicit settings JSON value and settings directory.
/// # Safety
/// `settings` and `preferences` must point to valid NUL-terminated UTF-8 strings for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn console_ui_create_with_settings(
    settings: *const c_char,
    preferences: *const c_char,
) -> *mut Bridge {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Bridge> {
        anyhow::ensure!(
            !settings.is_null() && !preferences.is_null(),
            "Null settings input"
        );
        let settings: Settings =
            serde_json::from_str(unsafe { CStr::from_ptr(settings) }.to_str()?)?;
        let path = std::path::PathBuf::from(unsafe { CStr::from_ptr(preferences) }.to_str()?);
        Ok(Bridge(Mutex::new(Workspace::with_preferences(
            settings, path, None, None,
        )?)))
    }));
    match result {
        Ok(Ok(bridge)) => Box::into_raw(Box::new(bridge)),
        Ok(Err(e)) => {
            error(format!("{e:#}"));
            std::ptr::null_mut()
        }
        Err(_) => {
            error("Native controller initialization failed");
            std::ptr::null_mut()
        }
    }
}
/// # Safety
/// `bridge` must be a live pointer returned by a create function. `action` must be a valid
/// NUL-terminated UTF-8 string. The caller must not destroy the controller during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn console_ui_dispatch(
    bridge: *mut Bridge,
    action: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        anyhow::ensure!(
            !bridge.is_null() && !action.is_null(),
            "Null controller/action input"
        );
        let command: Action = serde_json::from_str(unsafe { CStr::from_ptr(action) }.to_str()?)?;
        unsafe { &*bridge }
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Controller lock is unavailable"))?
            .dispatch(command)
    }));
    match result {
        Ok(Ok(())) => json(serde_json::json!({"ok":true})),
        Ok(Err(e)) => {
            error(format!("{e:#}"));
            json(serde_json::json!({"ok":false,"error":format!("{e:#}")}))
        }
        Err(_) => {
            error("Native controller action failed");
            json(serde_json::json!({"ok":false,"error":"Native controller action failed"}))
        }
    }
}
/// # Safety
/// `bridge` must remain live for this call and must not be concurrently destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn console_ui_view(bridge: *mut Bridge) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<serde_json::Value> {
        anyhow::ensure!(!bridge.is_null(), "Null controller");
        let app = unsafe { &*bridge }
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Controller lock is unavailable"))?;
        serde_json::to_value(app.view()?).context("Serialize native view")
    }));
    match result {
        Ok(Ok(value)) => json(value),
        Ok(Err(e)) => {
            error(format!("{e:#}"));
            json(serde_json::json!({"error":format!("{e:#}")}))
        }
        Err(_) => {
            error("Native view rendering failed");
            json(serde_json::json!({"error":"Native view rendering failed"}))
        }
    }
}
/// The pointer remains valid until the next error on this thread; copy it immediately.
#[unsafe(no_mangle)]
pub extern "C" fn console_ui_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}
/// # Safety
/// `value` must be null or an owned string returned by this ABI, released exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn console_ui_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}
/// # Safety
/// `bridge` must be null or a controller from a create function. Release exactly once after
/// all in-flight calls have finished. Destruction closes workers and flushes recordings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn console_ui_destroy(bridge: *mut Bridge) {
    if !bridge.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(bridge) })));
    }
}
