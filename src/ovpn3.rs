//! Safe-ish Rust wrapper over the OpenVPN3 core C ABI (native/ovpn_ffi.h).
//!
//! Core log lines and connection events are delivered from the OpenVPN worker
//! thread through C callbacks; we forward them onto an `mpsc` channel so the
//! caller can drive everything from one place. `connect()` blocks, so callers
//! run it on a dedicated thread and use [`Vpn::stop`] / [`Vpn::handle`] from
//! elsewhere to bring it down.

use anyhow::{Result, anyhow};
use std::ffi::{CStr, CString, c_void};
use std::os::raw::{c_char, c_int};
use std::ptr;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[repr(C)]
struct RawClient {
    _private: [u8; 0],
}

type LogCb = extern "C" fn(*mut c_void, *const c_char);
type EventCb = extern "C" fn(*mut c_void, c_int, c_int, *const c_char, *const c_char);

unsafe extern "C" {
    fn ovpn_client_new(user: *mut c_void, log_cb: LogCb, event_cb: EventCb) -> *mut RawClient;
    fn ovpn_client_free(client: *mut RawClient);
    fn ovpn_client_prepare(
        client: *mut RawClient,
        profile: *const c_char,
        username: *const c_char,
        password: *const c_char,
        peer_keys: *const *const c_char,
        peer_vals: *const *const c_char,
        peer_count: c_int,
        msg_out: *mut *mut c_char,
    ) -> c_int;
    fn ovpn_client_connect(client: *mut RawClient, msg_out: *mut *mut c_char) -> c_int;
    fn ovpn_client_stop(client: *mut RawClient);
    fn ovpn_platform() -> *mut c_char;
    fn ovpn_string_free(s: *mut c_char);
}

/// A connection event surfaced by the core (CONNECTED, DISCONNECTED, ...).
#[derive(Debug, Clone)]
pub struct Event {
    #[allow(dead_code)] // part of the event surface; not all fields are consumed yet
    pub error: bool,
    pub fatal: bool,
    pub name: String,
    pub info: String,
}

/// Messages forwarded from the OpenVPN worker thread.
#[derive(Debug, Clone)]
pub enum Msg {
    Log(String),
    Event(Event),
}

struct Sink {
    tx: UnboundedSender<Msg>,
}

/// Opaque, `Send` copy of the raw client pointer so `connect()`/`stop()` can run
/// on different threads. The core's ClientAPI explicitly permits calling
/// `stop()` from a thread other than the one running `connect()`.
#[derive(Clone, Copy)]
pub struct Handle(*mut RawClient);
unsafe impl Send for Handle {}

pub struct Vpn {
    raw: *mut RawClient,
    // Kept alive for the client's lifetime; the C side holds a pointer into it.
    _sink: Box<Sink>,
}

extern "C" fn log_trampoline(user: *mut c_void, text: *const c_char) {
    if user.is_null() || text.is_null() {
        return;
    }
    let sink = unsafe { &*(user as *const Sink) };
    let line = unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned();
    let _ = sink.tx.send(Msg::Log(line));
}

extern "C" fn event_trampoline(
    user: *mut c_void,
    error: c_int,
    fatal: c_int,
    name: *const c_char,
    info: *const c_char,
) {
    if user.is_null() {
        return;
    }
    let sink = unsafe { &*(user as *const Sink) };
    let to_str = |p: *const c_char| {
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    let _ = sink.tx.send(Msg::Event(Event {
        error: error != 0,
        fatal: fatal != 0,
        name: to_str(name),
        info: to_str(info),
    }));
}

impl Vpn {
    pub fn new() -> Result<(Vpn, UnboundedReceiver<Msg>)> {
        let (tx, rx) = unbounded_channel();
        let sink = Box::new(Sink { tx });
        let user = (&*sink as *const Sink) as *mut c_void;
        let raw = unsafe { ovpn_client_new(user, log_trampoline, event_trampoline) };
        if raw.is_null() {
            return Err(anyhow!("ovpn_client_new returned null"));
        }
        Ok((Vpn { raw, _sink: sink }, rx))
    }

    /// Parse the profile and install credentials + peer-info. Must be called
    /// before [`Vpn::connect`].
    pub fn prepare(
        &self,
        profile: &str,
        username: &str,
        password: &str,
        peer_info: &[(String, String)],
    ) -> Result<()> {
        let profile = CString::new(profile)?;
        let username = CString::new(username)?;
        let password = CString::new(password)?;

        let keys: Vec<CString> = peer_info
            .iter()
            .map(|(k, _)| CString::new(k.as_str()))
            .collect::<std::result::Result<_, _>>()?;
        let vals: Vec<CString> = peer_info
            .iter()
            .map(|(_, v)| CString::new(v.as_str()))
            .collect::<std::result::Result<_, _>>()?;
        let key_ptrs: Vec<*const c_char> = keys.iter().map(|s| s.as_ptr()).collect();
        let val_ptrs: Vec<*const c_char> = vals.iter().map(|s| s.as_ptr()).collect();

        let mut msg: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            ovpn_client_prepare(
                self.raw,
                profile.as_ptr(),
                username.as_ptr(),
                password.as_ptr(),
                key_ptrs.as_ptr(),
                val_ptrs.as_ptr(),
                key_ptrs.len() as c_int,
                &mut msg,
            )
        };
        check(rc, msg, "prepare")
    }

    pub fn handle(&self) -> Handle {
        Handle(self.raw)
    }

    pub fn stop(&self) {
        unsafe { ovpn_client_stop(self.raw) }
    }
}

impl Drop for Vpn {
    fn drop(&mut self) {
        unsafe { ovpn_client_free(self.raw) }
    }
}

/// Run the blocking connect loop for `handle`. Returns when the session ends.
pub fn run_connect(handle: Handle) -> Result<()> {
    let mut msg: *mut c_char = ptr::null_mut();
    let rc = unsafe { ovpn_client_connect(handle.0, &mut msg) };
    check(rc, msg, "connect")
}

pub fn platform() -> String {
    let p = unsafe { ovpn_platform() };
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { ovpn_string_free(p) };
    s
}

fn check(rc: c_int, msg: *mut c_char, what: &str) -> Result<()> {
    if rc == 0 {
        if !msg.is_null() {
            unsafe { ovpn_string_free(msg) };
        }
        return Ok(());
    }
    let detail = if msg.is_null() {
        String::new()
    } else {
        let s = unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned();
        unsafe { ovpn_string_free(msg) };
        s
    };
    Err(anyhow!("OpenVPN3 {what} failed: {detail}"))
}
