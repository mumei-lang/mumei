// =============================================================
// Plan 11: HTTP FFI Backend
// =============================================================
// Rust-side #[no_mangle] implementations for std/http.mm extern declarations.
// Uses reqwest (blocking) for HTTP operations.
// Response objects are stored in a global handle map (i64 → HttpResponse).

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::Mutex;

/// Stored HTTP response data
struct HttpResponse {
    status: u16,
    body: String,
    headers: HashMap<String, String>,
}

// NOTE: HTTP_STORE is append-only — handles are never removed.
// For long-running programs, this causes unbounded memory growth.
// TODO: Add `http_free(handle: i64)` FFI function to allow Mumei programs
// to release response handles when no longer needed.
lazy_static::lazy_static! {
    static ref HTTP_STORE: Mutex<HashMap<i64, HttpResponse>> = Mutex::new(HashMap::new());
    static ref NEXT_HTTP_HANDLE: Mutex<i64> = Mutex::new(1);
}

fn alloc_http_handle(resp: HttpResponse) -> i64 {
    let mut store = HTTP_STORE.lock().unwrap();
    let mut next = NEXT_HTTP_HANDLE.lock().unwrap();
    let handle = *next;
    *next += 1;
    store.insert(handle, resp);
    handle
}

// NOTE: Intentionally leaks CString. See src/ffi/json.rs alloc_string_result for details.
// TODO: Share a single alloc_string_result implementation or add mumei_str_free.
fn alloc_string_result(s: &str) -> *const c_char {
    match std::ffi::CString::new(s) {
        Ok(cs) => {
            let ptr = cs.as_ptr();
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

fn do_request(method: &str, url: &str, body: Option<&str>) -> i64 {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(_) => return 0,
    };

    let request = match method {
        "GET" => client.get(url),
        "POST" => {
            let mut req = client.post(url);
            if let Some(b) = body {
                req = req.body(b.to_string());
            }
            req
        }
        "PUT" => {
            let mut req = client.put(url);
            if let Some(b) = body {
                req = req.body(b.to_string());
            }
            req
        }
        "DELETE" => client.delete(url),
        _ => return 0,
    };

    match request.send() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let headers: HashMap<String, String> = resp
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            let body_text = resp.text().unwrap_or_default();
            alloc_http_handle(HttpResponse {
                status,
                body: body_text,
                headers,
            })
        }
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn http_get(url: *const c_char) -> i64 {
    let url_str = unsafe { c_str_to_str(url) };
    do_request("GET", url_str, None)
}

#[no_mangle]
pub extern "C" fn http_post(url: *const c_char, body: *const c_char) -> i64 {
    let url_str = unsafe { c_str_to_str(url) };
    let body_str = unsafe { c_str_to_str(body) };
    do_request("POST", url_str, Some(body_str))
}

#[no_mangle]
pub extern "C" fn http_put(url: *const c_char, body: *const c_char) -> i64 {
    let url_str = unsafe { c_str_to_str(url) };
    let body_str = unsafe { c_str_to_str(body) };
    do_request("PUT", url_str, Some(body_str))
}

#[no_mangle]
pub extern "C" fn http_delete(url: *const c_char) -> i64 {
    let url_str = unsafe { c_str_to_str(url) };
    do_request("DELETE", url_str, None)
}

#[no_mangle]
pub extern "C" fn http_status(handle: i64) -> i64 {
    let store = HTTP_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(resp) => resp.status as i64,
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn http_body(handle: i64) -> *const c_char {
    let store = HTTP_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(resp) => alloc_string_result(&resp.body),
        None => alloc_string_result(""),
    }
}

#[no_mangle]
pub extern "C" fn http_body_json(handle: i64) -> i64 {
    let store = HTTP_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(resp) => {
            let body = resp.body.clone();
            drop(store);
            // Delegate to json_parse from the json FFI module via C ABI
            let c_body = std::ffi::CString::new(body).unwrap_or_default();
            super::json::json_parse(c_body.as_ptr())
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn http_header_get(handle: i64, name: *const c_char) -> *const c_char {
    let name_str = unsafe { c_str_to_str(name) };
    let store = HTTP_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(resp) => {
            let val = resp.headers.get(name_str).map(|s| s.as_str()).unwrap_or("");
            alloc_string_result(val)
        }
        None => alloc_string_result(""),
    }
}

#[no_mangle]
pub extern "C" fn http_header_set(handle: i64, name: *const c_char, value: *const c_char) -> i64 {
    let name_str = unsafe { c_str_to_str(name) }.to_string();
    let value_str = unsafe { c_str_to_str(value) }.to_string();
    let mut store = HTTP_STORE.lock().unwrap();
    if let Some(resp) = store.get_mut(&handle) {
        resp.headers.insert(name_str, value_str);
        handle
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn http_is_ok(handle: i64) -> i64 {
    let store = HTTP_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(resp) => {
            if (200..300).contains(&resp.status) {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn http_is_error(handle: i64) -> i64 {
    let store = HTTP_STORE.lock().unwrap();
    match store.get(&handle) {
        Some(resp) => {
            if resp.status >= 400 {
                1
            } else {
                0
            }
        }
        None => 1, // handle 0 = network error
    }
}
