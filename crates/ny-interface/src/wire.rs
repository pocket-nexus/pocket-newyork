//! The guest's end of the channel for PocketJS's C hosts (the 3DS's `qjs.c`,
//! the portable `pocket_runtime.c`): they carry `ui.svcOpen`, `ui.svcPoll`
//! and `ui.svcSend` to the functions of `svcwire.h`, which their own builds
//! implement as a network client. An application that draws its own scene
//! links these instead, and the lines stay in the process.
use core::ffi::{c_char, CStr};

use crate::channel;

/// # Safety
/// `app` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn svcwire_open(app: *const c_char) -> bool {
    CStr::from_ptr(app).to_str().is_ok_and(|service| channel().open(service))
}

#[no_mangle]
pub extern "C" fn svcwire_pump() {}

/// # Safety
/// `out` has room for `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn svcwire_recv_lines(out: *mut u8, capacity: usize) -> usize {
    let Some(line) = channel().poll_within(capacity) else { return 0 };
    core::ptr::copy_nonoverlapping(line.as_ptr(), out, line.len());
    line.len()
}

/// # Safety
/// `line` points at `length` bytes.
#[no_mangle]
pub unsafe extern "C" fn svcwire_send_line(line: *const u8, length: usize) {
    if let Ok(line) = core::str::from_utf8(core::slice::from_raw_parts(line, length)) {
        channel().receive(line);
    }
}

#[no_mangle]
pub extern "C" fn svcwire_reset() {}

#[no_mangle]
pub extern "C" fn svcwire_shutdown() {
    unsafe { channel().close() }
}
