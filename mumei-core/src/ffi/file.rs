use std::fs;
use std::path::PathBuf;

fn handle_to_path(handle: i64) -> Option<PathBuf> {
    if handle <= 0 {
        return None;
    }
    super::json::mumei_str_clone(handle).map(PathBuf::from)
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
    let Some(content) = super::json::mumei_str_clone(content) else {
        return 0;
    };
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
