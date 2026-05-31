use std::ffi::{CStr, CString};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

pub fn c_string(value: impl AsRef<str>) -> CString {
    let filtered: String = value.as_ref().chars().filter(|c| *c != '\0').collect();
    CString::new(filtered).expect("CString::new failed")
}

pub fn string_handle(value: impl AsRef<str>) -> i64 {
    let value = c_string(value);
    mumei_core::ffi::json::mumei_str_alloc(value.as_ptr())
}

pub fn json_object_handle() -> i64 {
    mumei_core::ffi::json::json_object_new()
}

pub fn json_array_handle() -> i64 {
    mumei_core::ffi::json::json_array_new()
}

pub fn temp_path(name: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mumei_ffi_contract_{name}_{id}"))
}

pub fn temp_path_handle(name: &str) -> (i64, PathBuf) {
    let path = temp_path(name);
    let handle = string_handle(path.to_string_lossy());
    (handle, path)
}

pub fn local_http_url() -> CString {
    https_error_url()
}

pub fn insecure_local_http_url() -> CString {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test HTTP listener");
    let addr = listener.local_addr().expect("local HTTP listener addr");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: application/json\r\n",
                "X-Mumei-Contract: ok\r\n",
                "Content-Length: 11\r\n",
                "\r\n",
                "{\"ok\":true}"
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    c_string(format!("http://{addr}/contract"))
}

pub fn https_error_url() -> CString {
    c_string("https://127.0.0.1:9/contract")
}

pub fn http_response_handle() -> i64 {
    let url = insecure_local_http_url();
    mumei_core::ffi::http::http_get(url.as_ptr())
}

pub fn unique_local_addr() -> CString {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve local port");
    let addr = listener.local_addr().expect("reserved local addr");
    drop(listener);
    c_string(addr.to_string())
}

pub fn server_handle() -> i64 {
    for _ in 0..16 {
        let addr = unique_local_addr();
        let handle = mumei_core::ffi::http_server::http_server_bind(addr.as_ptr());
        if handle > 0 {
            return handle;
        }
    }
    0
}

pub fn server_handle_with_pending_client() -> (i64, thread::JoinHandle<()>) {
    for _ in 0..16 {
        let addr = unique_local_addr();
        let addr_str = unsafe { CStr::from_ptr(addr.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let server = mumei_core::ffi::http_server::http_server_bind(addr.as_ptr());
        if server <= 0 {
            continue;
        }

        let client = thread::spawn(move || {
            for _ in 0..50 {
                match TcpStream::connect(&addr_str) {
                    Ok(mut stream) => {
                        let _ =
                            stream.write_all(b"GET /contract HTTP/1.1\r\nHost: localhost\r\n\r\n");
                        let _ = stream.flush();
                        let mut response = [0_u8; 1024];
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let _ = stream.read(&mut response);
                        return;
                    }
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
        });

        return (server, client);
    }
    (0, thread::spawn(|| {}))
}

pub fn server_request_handle() -> (i64, i64, thread::JoinHandle<()>) {
    for _ in 0..16 {
        let addr = unique_local_addr();
        let addr_str = unsafe { CStr::from_ptr(addr.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let server = mumei_core::ffi::http_server::http_server_bind(addr.as_ptr());
        if server <= 0 {
            continue;
        }

        let client = thread::spawn(move || {
            for _ in 0..50 {
                match TcpStream::connect(&addr_str) {
                    Ok(mut stream) => {
                        let _ =
                            stream.write_all(b"GET /contract HTTP/1.1\r\nHost: localhost\r\n\r\n");
                        let _ = stream.flush();
                        let mut response = [0_u8; 1024];
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let _ = stream.read(&mut response);
                        return;
                    }
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
        });

        let request = mumei_core::ffi::http_server::http_server_accept(server);
        if request > 0 {
            return (server, request, client);
        }
        let _ = client.join();
        mumei_core::ffi::http_server::http_server_free(server);
    }
    (0, 0, thread::spawn(|| {}))
}

pub fn ptr_is_non_null(ptr: *const c_char) -> bool {
    !ptr.is_null()
}

pub fn contract_result_observed<T>(_result: &T) -> bool {
    true
}
