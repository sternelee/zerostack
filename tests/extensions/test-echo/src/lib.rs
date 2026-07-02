//! Test echo plugin — compiled to wasm32-unknown-unknown with panic=abort.
//!
//! Exports: alloc, init, tool_execute
//! Imports: host_register_tool (from "env" module)

use core::ptr::{addr_of_mut, copy_nonoverlapping};

unsafe extern "C" {
    #[link_name = "host_register_tool"]
    fn host_register_tool(def_ptr: *const u8, def_len: usize) -> i32;
}

static mut BUMP: [u8; 65536] = [0; 65536];
static mut BUMP_OFF: usize = 0;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alloc(size: usize) -> *mut u8 {
    let off = addr_of_mut!(BUMP_OFF).read_volatile();
    let aligned = (off + 3) & !3;
    if aligned + size > BUMP.len() {
        return core::ptr::null_mut();
    }
    addr_of_mut!(BUMP_OFF).write_volatile(aligned + size);
    addr_of_mut!(BUMP).cast::<u8>().add(aligned)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn init() -> i32 {
    let def = b"{\"name\":\"echo\",\"label\":\"Echo\",\"description\":\"Echo back the input message.\",\"parameters_schema\":\"{\\\"type\\\":\\\"object\\\",\\\"properties\\\":{\\\"message\\\":{\\\"type\\\":\\\"string\\\",\\\"description\\\":\\\"Message to echo\\\"}},\\\"required\\\":[\\\"message\\\"]}\",\"prompt_snippet\":\"Echo back a message\",\"prompt_guidelines\":[]}";
    host_register_tool(def.as_ptr(), def.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tool_execute(
    _name_ptr: *const u8,
    _name_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> *const u8 {
    let len = if params_len > 256 { 256 } else { params_len };
    let params = core::slice::from_raw_parts(params_ptr, len);
    let s = core::str::from_utf8(params).unwrap_or("{}");

    let prefix = b"{\"content\":\"echo: ";
    let suffix = b"\",\"details\":\"{}\",\"is_error\":false}";
    let total = prefix.len() + s.len().min(200) + suffix.len();

    let dst = alloc(total);
    if dst.is_null() {
        return b"err\0".as_ptr();
    }

    let mut p = dst;
    copy_nonoverlapping(prefix.as_ptr(), p, prefix.len());
    p = p.add(prefix.len());
    copy_nonoverlapping(s.as_ptr(), p, s.len().min(200));
    p = p.add(s.len().min(200));
    copy_nonoverlapping(suffix.as_ptr(), p, suffix.len());
    dst
}
