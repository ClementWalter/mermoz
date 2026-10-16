//! In-process mobile access to the local mailbox through an owned C ABI.
//!
//! Calls on a handle must be serialized; the app owns its sandbox path and
//! foreground sync lifecycle. No daemon or Mac connection is required.

mod engine;
mod login;

pub use engine::MobileEngine;
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

fn owned_string(value: Value) -> *mut c_char {
    // JSON escapes embedded NULs, so the owned C string always has one terminator.
    CString::new(value.to_string())
        .expect("JSON contains no NUL")
        .into_raw()
}

fn response(result: Result<Value, String>) -> *mut c_char {
    owned_string(match result {
        Ok(value) => json!({ "result": value }),
        Err(error) => json!({ "error": error }),
    })
}

/// Opens a persistent demo store; the caller owns the handle and any error string.
///
/// # Safety
/// `home` must be a NUL-terminated UTF-8 string. `error` must point to a writable
/// pointer slot. Release errors with `mermoz_mobile_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mermoz_mobile_open(
    home: *const c_char,
    error: *mut *mut c_char,
) -> *mut MobileEngine {
    if error.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { *error = std::ptr::null_mut() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if home.is_null() {
            return Err("Missing mailbox directory".to_owned());
        }
        let home = unsafe { CStr::from_ptr(home) }
            .to_str()
            .map_err(|e| e.to_string())?;
        MobileEngine::open(Path::new(home)).map_err(|e| e.to_string())
    }))
    .unwrap_or_else(|_| Err("The local mailbox could not be opened".into()));
    match result {
        Ok(engine) => Box::into_raw(Box::new(engine)),
        Err(message) => {
            unsafe { *error = CString::new(message.replace('\0', " ")).unwrap().into_raw() };
            std::ptr::null_mut()
        }
    }
}

/// Executes a JSON request and returns an owned JSON result or error envelope.
///
/// # Safety
/// `engine` must be a live handle from `mermoz_mobile_open`, used serially.
/// `request` must be a NUL-terminated UTF-8 string. Free the response once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mermoz_mobile_request(
    engine: *mut MobileEngine,
    request: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if engine.is_null() || request.is_null() {
            return Err("Missing engine or request".to_owned());
        }
        let request = unsafe { CStr::from_ptr(request) }
            .to_str()
            .map_err(|e| e.to_string())?;
        unsafe { &*engine }.dispatch(request)
    }))
    .unwrap_or_else(|_| Err("The local mailbox request failed".into()));
    response(result)
}

/// Releases a handle after all requests have finished; a null handle is ignored.
///
/// # Safety
/// A non-null handle must come from `mermoz_mobile_open` and be released once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mermoz_mobile_close(engine: *mut MobileEngine) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}

/// Releases a returned string; a null string is ignored.
///
/// # Safety
/// A non-null string must come from this ABI and be released once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mermoz_mobile_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

#[cfg(test)]
mod tests;
