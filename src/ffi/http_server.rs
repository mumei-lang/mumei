// =============================================================
// HTTP Server FFI Backend
// =============================================================
// Rust-side #[no_mangle] implementations for std/http_server.mm extern declarations.
// Uses std::net::TcpListener for minimal HTTP server (no external dependencies).
// Server and request objects are stored in global handle maps (i64 → handle).

use std::collections::HashMap;
use std::ffi::CStr;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::raw::c_char;
use std::sync::Mutex;

/// Stored HTTP server handle
struct ServerHandle {
    listener: TcpListener,
}

/// Stored HTTP request data (parsed from incoming connection)
struct RequestHandle {
    method: String,
    path: String,
    stream: Option<std::net::TcpStream>,
}

lazy_static::lazy_static! {
    static ref SERVER_STORE: Mutex<HashMap<i64, ServerHandle>> = Mutex::new(HashMap::new());
    static ref REQUEST_STORE: Mutex<HashMap<i64, RequestHandle>> = Mutex::new(HashMap::new());
    static ref NEXT_SERVER_HANDLE: Mutex<i64> = Mutex::new(1);
    static ref NEXT_REQUEST_HANDLE: Mutex<i64> = Mutex::new(1);
}

fn alloc_string_result(s: &str) -> *const c_char {
    super::json::alloc_string_result(s)
}

unsafe fn c_str_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    CStr::from_ptr(ptr).to_str().unwrap_or("")
}

/// Bind a TcpListener to the given address. Returns a server handle (>0) or 0 on error.
#[no_mangle]
pub extern "C" fn http_server_bind(addr: *const c_char) -> i64 {
    let addr_str = unsafe { c_str_to_str(addr) };
    match TcpListener::bind(addr_str) {
        Ok(listener) => {
            let mut store = SERVER_STORE.lock().unwrap();
            let mut next = NEXT_SERVER_HANDLE.lock().unwrap();
            let handle = *next;
            *next += 1;
            store.insert(handle, ServerHandle { listener });
            handle
        }
        Err(_) => 0,
    }
}

/// Accept a connection, parse the HTTP request line, return a request handle.
/// Blocks until a connection arrives. Returns 0 on error.
#[no_mangle]
pub extern "C" fn http_server_accept(server_handle: i64) -> i64 {
    let listener = {
        let store = SERVER_STORE.lock().unwrap();
        match store.get(&server_handle) {
            Some(sh) => match sh.listener.try_clone() {
                Ok(l) => l,
                Err(_) => return 0,
            },
            None => return 0,
        }
    };

    match listener.accept() {
        Ok((stream, _addr)) => {
            // Parse HTTP request line: "METHOD /path HTTP/1.x"
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(s) => s,
                Err(_) => return 0,
            });
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                return 0;
            }

            let parts: Vec<&str> = request_line.split_whitespace().collect();
            let (method, path) = if parts.len() >= 2 {
                (parts[0].to_string(), parts[1].to_string())
            } else {
                ("GET".to_string(), "/".to_string())
            };

            let mut req_store = REQUEST_STORE.lock().unwrap();
            let mut next = NEXT_REQUEST_HANDLE.lock().unwrap();
            let handle = *next;
            *next += 1;
            req_store.insert(
                handle,
                RequestHandle {
                    method,
                    path,
                    stream: Some(stream),
                },
            );
            handle
        }
        Err(_) => 0,
    }
}

/// Return the request path as a C string.
#[no_mangle]
pub extern "C" fn http_request_path(req_handle: i64) -> *const c_char {
    let store = REQUEST_STORE.lock().unwrap();
    match store.get(&req_handle) {
        Some(req) => alloc_string_result(&req.path),
        None => alloc_string_result(""),
    }
}

/// Return the request method as a C string.
#[no_mangle]
pub extern "C" fn http_request_method(req_handle: i64) -> *const c_char {
    let store = REQUEST_STORE.lock().unwrap();
    match store.get(&req_handle) {
        Some(req) => alloc_string_result(&req.method),
        None => alloc_string_result(""),
    }
}

/// Write an HTTP response with the given status code and body.
/// Returns 1 on success, 0 on failure.
#[no_mangle]
pub extern "C" fn http_server_respond(req_handle: i64, status: i64, body: *const c_char) -> i64 {
    let body_str = unsafe { c_str_to_str(body) };
    let mut store = REQUEST_STORE.lock().unwrap();
    if let Some(req) = store.get_mut(&req_handle) {
        // Take the stream so that a second call to respond on the same handle
        // will find None and return 0 (single-response enforcement at runtime).
        if let Some(mut stream) = req.stream.take() {
            let status_text = match status {
                200 => "OK",
                201 => "Created",
                204 => "No Content",
                400 => "Bad Request",
                403 => "Forbidden",
                404 => "Not Found",
                500 => "Internal Server Error",
                _ => "Unknown",
            };
            let response = format!(
                "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
                status,
                status_text,
                body_str.len(),
                body_str
            );
            if stream.write_all(response.as_bytes()).is_ok() {
                let _ = stream.flush();
                return 1;
            }
        }
    }
    0
}

/// Free a server handle from SERVER_STORE.
/// Returns 1 if found and removed, 0 otherwise.
#[no_mangle]
pub extern "C" fn http_server_free(server_handle: i64) -> i64 {
    let mut store = SERVER_STORE.lock().unwrap();
    if store.remove(&server_handle).is_some() {
        1
    } else {
        0
    }
}

/// Free a request handle from REQUEST_STORE.
/// Returns 1 if found and removed, 0 otherwise.
#[no_mangle]
pub extern "C" fn http_request_free(req_handle: i64) -> i64 {
    let mut store = REQUEST_STORE.lock().unwrap();
    if store.remove(&req_handle).is_some() {
        1
    } else {
        0
    }
}
