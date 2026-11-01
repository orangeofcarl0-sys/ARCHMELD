// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Custom DACL for the ARCHMELD named-pipe listener.
//!
//! Tokio's `ServerOptions` creates a pipe with the process default DACL. B028
//! asks for an explicit pipe ACL, so the first and every subsequent instance is
//! created with `CreateNamedPipeW` and a protected DACL that grants full access
//! only to the current user, SYSTEM and the Administrators group. The bearer
//! token check remains in the HTTP middleware.

use serde_json::{json, Value};
use std::ffi::{c_void, OsStr};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::RawHandle;
use std::ptr::null_mut;
use tokio::net::windows::named_pipe::NamedPipeServer;

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
const TOKEN_QUERY: u32 = 0x0000_0008;
const TOKEN_USER_CLASS: u32 = 1;
const SDDL_REVISION_1: u32 = 1;
const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
const SE_KERNEL_OBJECT: u32 = 6;

#[repr(C)]
struct SecurityAttributes {
    n_length: u32,
    lp_security_descriptor: *mut c_void,
    b_inherit_handle: i32,
}

#[repr(C)]
struct SidAndAttributes {
    sid: *mut c_void,
    attributes: u32,
}

#[repr(C)]
struct TokenUser {
    user: SidAndAttributes,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buffer_size: u32,
        in_buffer_size: u32,
        default_timeout: u32,
        security_attributes: *mut SecurityAttributes,
    ) -> *mut c_void;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(
        process: *mut c_void,
        desired_access: u32,
        token: *mut *mut c_void,
    ) -> i32;
    fn GetTokenInformation(
        token: *mut c_void,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn ConvertSidToStringSidW(sid: *mut c_void, string_sid: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn ConvertSecurityDescriptorToStringSecurityDescriptorW(
        descriptor: *mut c_void,
        revision: u32,
        information: u32,
        string: *mut *mut u16,
        length: *mut u32,
    ) -> i32;
    fn GetSecurityInfo(
        handle: *mut c_void,
        object_type: u32,
        information: u32,
        owner: *mut *mut c_void,
        group: *mut *mut c_void,
        dacl: *mut *mut c_void,
        sacl: *mut *mut c_void,
        descriptor: *mut *mut c_void,
    ) -> u32;
}

pub struct PipeSecurity {
    pub sddl: String,
    sid: String,
}

impl PipeSecurity {
    pub fn for_current_user() -> io::Result<Self> {
        let sid = current_user_sid()?;
        Ok(Self {
            sddl: format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{sid})"),
            sid,
        })
    }
}

fn current_user_sid() -> io::Result<String> {
    // SAFETY: every handle and allocation returned by the Win32 calls is
    // checked and released on the path that owns it; the token buffer is a
    // usize vector so TokenUser/SID pointers stay aligned.
    unsafe {
        let mut token: *mut c_void = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut needed = 0u32;
        GetTokenInformation(token, TOKEN_USER_CLASS, null_mut(), 0, &mut needed);
        if needed == 0 {
            let error = io::Error::last_os_error();
            CloseHandle(token);
            return Err(error);
        }
        let words = (needed as usize + size_of::<usize>() - 1) / size_of::<usize>();
        let mut buffer = vec![0usize; words];
        let mut returned = 0u32;
        if GetTokenInformation(
            token,
            TOKEN_USER_CLASS,
            buffer.as_mut_ptr() as *mut c_void,
            needed,
            &mut returned,
        ) == 0
        {
            let error = io::Error::last_os_error();
            CloseHandle(token);
            return Err(error);
        }
        let user = &*(buffer.as_ptr() as *const TokenUser);
        let mut text: *mut u16 = null_mut();
        if ConvertSidToStringSidW(user.user.sid, &mut text) == 0 {
            let error = io::Error::last_os_error();
            CloseHandle(token);
            return Err(error);
        }
        let mut length = 0usize;
        while *text.add(length) != 0 {
            length += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text as *mut c_void);
        CloseHandle(token);
        Ok(sid)
    }
}

pub fn create(name: &str, security: &PipeSecurity, first: bool) -> io::Result<NamedPipeServer> {
    let sddl: Vec<u16> = security.sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor: *mut c_void = null_mut();
    let mut descriptor_size = 0u32;
    // SAFETY: the SDDL buffer is NUL-terminated and outlives the call; the
    // returned descriptor is released after CreateNamedPipeW consumed it.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut descriptor_size,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut attributes = SecurityAttributes {
            n_length: size_of::<SecurityAttributes>() as u32,
            lp_security_descriptor: descriptor,
            b_inherit_handle: 0,
        };
        let pipe_name: Vec<u16> = OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let open_mode = PIPE_ACCESS_DUPLEX
            | FILE_FLAG_OVERLAPPED
            | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let handle = CreateNamedPipeW(
            pipe_name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &mut attributes,
        );
        LocalFree(descriptor);
        if handle == null_mut() || handle as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        match NamedPipeServer::from_raw_handle(handle as RawHandle) {
            Ok(server) => Ok(server),
            Err(error) => {
                CloseHandle(handle);
                Err(error)
            }
        }
    }
}

/// Read the DACL back from the created pipe handle and report what an
/// external caller would see. This is evidence, not a substitute for the
/// kernel's access check.
pub fn describe(handle: RawHandle, security: &PipeSecurity) -> io::Result<Value> {
    let mut descriptor: *mut c_void = null_mut();
    let mut dacl: *mut c_void = null_mut();
    // SAFETY: the handle stays owned by the listener for the duration of the
    // call; GetSecurityInfo allocates the descriptor, freed below on success.
    unsafe {
        let status = GetSecurityInfo(
            handle as *mut c_void,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let mut text: *mut u16 = null_mut();
        let mut length = 0u32;
        let converted = ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            &mut length,
        );
        if converted == 0 {
            let error = io::Error::last_os_error();
            LocalFree(descriptor);
            return Err(error);
        }
        let mut len = 0usize;
        while *text.add(len) != 0 {
            len += 1;
        }
        let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text as *mut c_void);
        LocalFree(descriptor);
        let digest = archmeld_core::hash::digest_bytes(sddl.as_bytes());
        Ok(json!({
            "type": "custom_dacl",
            "verified": true,
            "protected": sddl.starts_with("D:P"),
            "everyone": sddl.contains(";;;WD)"),
            "anonymous": sddl.contains(";;;AN)"),
            "current_user": sddl.contains(security.sid.as_str()),
            "ace_count": sddl.matches("(A;").count(),
            "dacl_digest": digest,
            "readback": "GetSecurityInfo + ConvertSecurityDescriptorToStringSecurityDescriptorW",
        }))
    }
}
