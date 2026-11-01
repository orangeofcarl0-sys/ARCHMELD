// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Process memory ceiling for import commands.
//!
//! `--memory-limit-mb` applies a hard, OS-enforced limit before the trace is
//! read. On Windows the current process is placed in a job object with
//! `JOB_OBJECT_LIMIT_PROCESS_MEMORY`/`JOB_OBJECT_LIMIT_JOB_MEMORY`; on Linux and
//! macOS `RLIMIT_AS` is lowered. Exceeding the ceiling makes the allocation or
//! the process fail instead of letting the run silently exceed its budget.

use std::ffi::c_void;

const MB: u64 = 1024 * 1024;
const MAX_LIMIT_MB: u64 = 1_048_576;

fn limit_bytes(mb: u64) -> Result<u64, String> {
    if mb == 0 || mb > MAX_LIMIT_MB {
        return Err(format!("memory limit must be 1..={MAX_LIMIT_MB} MB"));
    }
    mb.checked_mul(MB)
        .ok_or_else(|| "memory limit is too large".to_string())
}

#[cfg(windows)]
pub fn apply_limit_mb(mb: u64) -> Result<(), String> {
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    type Handle = *mut c_void;
    #[repr(C)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }
    #[repr(C)]
    struct ExtendedLimitInformation {
        basic: BasicLimitInformation,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }
    const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x0000_0100;
    const JOB_OBJECT_LIMIT_JOB_MEMORY: u32 = 0x0000_0200;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            class: u32,
            information: *mut c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn GetCurrentProcess() -> Handle;
    }

    let bytes = usize::try_from(limit_bytes(mb)?)
        .map_err(|_| "memory limit does not fit this platform".to_string())?;
    unsafe {
        let job = CreateJobObjectW(null_mut(), null());
        if job.is_null() {
            return Err(format!(
                "CreateJobObjectW failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut information: ExtendedLimitInformation = zeroed();
        information.basic.limit_flags =
            JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_JOB_MEMORY;
        information.process_memory_limit = bytes;
        information.job_memory_limit = bytes;
        if SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            &mut information as *mut _ as *mut c_void,
            size_of::<ExtendedLimitInformation>() as u32,
        ) == 0
        {
            return Err(format!(
                "SetInformationJobObject failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            return Err(format!(
                "AssignProcessToJobObject failed (the process may already be in a non-breakaway job): {}",
                std::io::Error::last_os_error()
            ));
        }
        // The job must outlive this call: keep the handle open for the process
        // lifetime. Closing it would not remove the limits, but leaking one
        // handle is simpler and auditable.
        std::mem::forget(job);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn apply_limit_mb(mb: u64) -> Result<(), String> {
    const RLIMIT_AS: i32 = 9;
    #[repr(C)]
    struct RLimit {
        current: u64,
        maximum: u64,
    }
    extern "C" {
        fn setrlimit(resource: i32, limit: *const RLimit) -> i32;
    }
    let bytes = limit_bytes(mb)?;
    let limit = RLimit {
        current: bytes,
        maximum: bytes,
    };
    let code = unsafe { setrlimit(RLIMIT_AS, &limit) };
    if code != 0 {
        return Err(format!(
            "setrlimit(RLIMIT_AS) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn apply_limit_mb(mb: u64) -> Result<(), String> {
    const RLIMIT_AS: i32 = 5;
    #[repr(C)]
    struct RLimit {
        current: u64,
        maximum: u64,
    }
    extern "C" {
        fn setrlimit(resource: i32, limit: *const RLimit) -> i32;
    }
    let bytes = limit_bytes(mb)?;
    let limit = RLimit {
        current: bytes,
        maximum: bytes,
    };
    let code = unsafe { setrlimit(RLIMIT_AS, &limit) };
    if code != 0 {
        return Err(format!(
            "setrlimit(RLIMIT_AS) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn apply_limit_mb(_mb: u64) -> Result<(), String> {
    Err("hard memory limits are not implemented on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::limit_bytes;

    #[test]
    fn limit_validation_rejects_zero_and_overflow() {
        assert!(limit_bytes(0).is_err());
        assert!(limit_bytes(1_048_577).is_err());
        assert_eq!(limit_bytes(512).unwrap(), 512 * 1024 * 1024);
    }
}
