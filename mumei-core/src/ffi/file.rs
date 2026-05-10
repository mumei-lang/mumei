use std::fs;
use std::path::PathBuf;

fn handle_to_path(handle: i64) -> Option<PathBuf> {
    if handle <= 0 {
        return None;
    }
    let ptr = super::json::mumei_str_get(handle);
    if ptr.is_null() {
        return None;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_str()
        .ok()?
        .to_owned();
    Some(PathBuf::from(path))
}

#[no_mangle]
pub extern "C" fn file_read(path: i64) -> i64 {
    let Some(path) = handle_to_path(path) else {
        return 0;
    };
    match fs::read_to_string(path) {
        Ok(content) => super::json::mumei_str_alloc_internal(&content),
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn file_write(path: i64, content: i64) -> i64 {
    let Some(path) = handle_to_path(path) else {
        return 0;
    };
    let content_ptr = super::json::mumei_str_get(content);
    if content_ptr.is_null() {
        return 0;
    }
    let content = unsafe { std::ffi::CStr::from_ptr(content_ptr) }
        .to_str()
        .unwrap_or("");
    if fs::write(path, content).is_ok() {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn file_exists(path: i64) -> i64 {
    let Some(path) = handle_to_path(path) else {
        return 0;
    };
    if path.exists() {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn file_delete(path: i64) -> i64 {
    let Some(path) = handle_to_path(path) else {
        return 0;
    };
    if fs::remove_file(path).is_ok() {
        1
    } else {
        0
    }
}
