// =============================================================
// Plan 10: JSON FFI Backend
// =============================================================
// Rust-side #[no_mangle] implementations for std/json.mm extern declarations.
// Uses serde_json for JSON parsing/manipulation.
// JSON values are stored in a global handle map (i64 → serde_json::Value).

use serde_json::Value;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

lazy_static::lazy_static! {
    static ref JSON_STORE: Mutex<HashMap<i64, Value>> = Mutex::new(HashMap::new());
    static ref NEXT_JSON_HANDLE: Mutex<i64> = Mutex::new(1);
    static ref STRING_STORE: Mutex<HashMap<i64, CString>> = Mutex::new(HashMap::new());
    static ref NEXT_STRING_HANDLE: Mutex<i64> = Mutex::new(1);
}

fn alloc_json_handle(val: Value) -> i64 {
    let mut store = JSON_STORE.lock().unwrap();
    let mut next = NEXT_JSON_HANDLE.lock().unwrap();
    let handle = *next;
    *next += 1;
    store.insert(handle, val);
    handle
}

fn alloc_string_result(s: &str) -> *const c_char {
    match CString::new(s) {
        Ok(cs) => {
            let ptr = cs.as_ptr();
            // Leak the CString to keep the pointer valid
            std::mem::forget(cs);
            ptr
        }
        Err(_) => std::ptr::null(),
    }
}

unsafe fn c_str_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    CStr::from_ptr(ptr).to_str().unwrap_or("")
}

#[no_mangle]
pub extern "C" fn json_parse(input: *const c_char) -> i64 {
    let input_str = unsafe { c_str_to_str(input) };
    match serde_json::from_str::<Value>(input_str) {
        Ok(val) => alloc_json_handle(val),
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn json_stringify(handle: i64) -> *const c_char {
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => alloc_string_result(&val.to_string()),
        None => alloc_string_result("null"),
    }
}

#[no_mangle]
pub extern "C" fn json_get(handle: i64, key: *const c_char) -> i64 {
    let key_str = unsafe { c_str_to_str(key) };
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => {
            if let Some(child) = val.get(key_str) {
                let child_clone = child.clone();
                drop(store);
                alloc_json_handle(child_clone)
            } else {
                0
            }
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn json_get_int(handle: i64, key: *const c_char) -> i64 {
    let key_str = unsafe { c_str_to_str(key) };
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => val.get(key_str).and_then(|v| v.as_i64()).unwrap_or(0),
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn json_get_str(handle: i64, key: *const c_char) -> *const c_char {
    let key_str = unsafe { c_str_to_str(key) };
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => {
            let s = val.get(key_str).and_then(|v| v.as_str()).unwrap_or("");
            alloc_string_result(s)
        }
        None => alloc_string_result(""),
    }
}

#[no_mangle]
pub extern "C" fn json_get_bool(handle: i64, key: *const c_char) -> i64 {
    let key_str = unsafe { c_str_to_str(key) };
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => val
            .get(key_str)
            .and_then(|v| v.as_bool())
            .map(|b| if b { 1 } else { 0 })
            .unwrap_or(0),
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn json_array_len(handle: i64) -> i64 {
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => val.as_array().map(|a| a.len() as i64).unwrap_or(0),
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn json_array_get(handle: i64, index: i64) -> i64 {
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => {
            if let Some(arr) = val.as_array() {
                if let Some(elem) = arr.get(index as usize) {
                    let elem_clone = elem.clone();
                    drop(store);
                    return alloc_json_handle(elem_clone);
                }
            }
            0
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn json_is_null(handle: i64) -> i64 {
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => {
            if val.is_null() {
                1
            } else {
                0
            }
        }
        None => 1, // handle 0 or invalid = null
    }
}

#[no_mangle]
pub extern "C" fn json_is_object(handle: i64) -> i64 {
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => {
            if val.is_object() {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn json_is_array(handle: i64) -> i64 {
    let store = JSON_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(val) => {
            if val.is_array() {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn json_object_new() -> i64 {
    alloc_json_handle(Value::Object(serde_json::Map::new()))
}

#[no_mangle]
pub extern "C" fn json_object_set(handle: i64, key: *const c_char, value: i64) -> i64 {
    let key_str = unsafe { c_str_to_str(key) }.to_string();
    let mut store = JSON_STORE.lock().unwrap();
    let val_to_set = store.get(&value).cloned().unwrap_or(Value::Null);
    if let Some(obj) = store.get_mut(&handle) {
        if let Some(map) = obj.as_object_mut() {
            map.insert(key_str, val_to_set);
            return handle;
        }
    }
    0
}

#[no_mangle]
pub extern "C" fn json_array_new() -> i64 {
    alloc_json_handle(Value::Array(Vec::new()))
}

#[no_mangle]
pub extern "C" fn json_array_push(handle: i64, value: i64) -> i64 {
    let mut store = JSON_STORE.lock().unwrap();
    let val_to_push = store.get(&value).cloned().unwrap_or(Value::Null);
    if let Some(arr_val) = store.get_mut(&handle) {
        if let Some(arr) = arr_val.as_array_mut() {
            arr.push(val_to_push);
            return handle;
        }
    }
    0
}

#[no_mangle]
pub extern "C" fn json_from_int(value: i64) -> i64 {
    alloc_json_handle(Value::Number(serde_json::Number::from(value)))
}

#[no_mangle]
pub extern "C" fn json_from_str(value: *const c_char) -> i64 {
    let s = unsafe { c_str_to_str(value) };
    alloc_json_handle(Value::String(s.to_string()))
}

#[no_mangle]
pub extern "C" fn json_from_bool(value: i64) -> i64 {
    alloc_json_handle(Value::Bool(value != 0))
}

/// Plan 9-8: String concatenation runtime helper
#[no_mangle]
pub extern "C" fn mumei_str_concat(a: *const c_char, b: *const c_char) -> *const c_char {
    let a_str = unsafe { c_str_to_str(a) };
    let b_str = unsafe { c_str_to_str(b) };
    let result = format!("{}{}", a_str, b_str);
    alloc_string_result(&result)
}
