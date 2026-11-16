//! Per-operation native allocation containment. Called only in the isolated clipboard worker.
use anyhow::{Result, ensure};
const NATIVE_GROWTH: u64 = 96 * 1024 * 1024;
const DECODE_GROWTH: u64 = 1024 * 1024 * 1024;
#[cfg(unix)]
pub(super) struct Budget {
    baseline: u64,
    hard: libc::rlim_t,
}
#[cfg(unix)]
fn virtual_bytes() -> Result<u64> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let mut bytes = String::new();
        std::fs::File::open("/proc/self/statm")?
            .take(4096)
            .read_to_string(&mut bytes)?;
        let pages = bytes
            .split_whitespace()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Native memory size unavailable"))?
            .parse::<u64>()?;
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        ensure!(page > 0, "Native page size unavailable");
        Ok(pages
            .checked_mul(page as u64)
            .ok_or_else(|| anyhow::anyhow!("Native memory size overflow"))?)
    }
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct Info {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_seconds: i32,
            user_micros: i32,
            system_seconds: i32,
            system_micros: i32,
            policy: i32,
            suspend: i32,
        }
        unsafe extern "C" {
            static mach_task_self_: u32;
            fn task_info(task: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
        }
        let mut info: Info = unsafe { std::mem::zeroed() };
        let mut count = (std::mem::size_of::<Info>() / 4) as u32;
        ensure!(
            unsafe {
                task_info(
                    mach_task_self_,
                    20,
                    (&mut info as *mut Info).cast(),
                    &mut count,
                )
            } == 0
                && count as usize >= std::mem::size_of::<Info>() / 4,
            "Native VM accounting unavailable"
        );
        Ok(info.virtual_size)
    }
}
#[cfg(unix)]
impl Budget {
    pub fn enter() -> Result<Self> {
        let baseline = virtual_bytes()?;
        let mut previous: libc::rlimit = unsafe { std::mem::zeroed() };
        ensure!(
            unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut previous) } == 0,
            "Could not inspect native VM ceiling"
        );
        let hard = previous.rlim_max.min(
            baseline
                .checked_add(DECODE_GROWTH)
                .ok_or_else(|| anyhow::anyhow!("Native VM bound overflow"))?
                as libc::rlim_t,
        );
        let value = Self { baseline, hard };
        value.limit(NATIVE_GROWTH)?;
        let small = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        ensure!(
            small != libc::MAP_FAILED,
            "Native bounded allocation unavailable"
        );
        unsafe {
            libc::munmap(small, 4096);
        }
        let length = (NATIVE_GROWTH + 65536) as usize;
        let large = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if large != libc::MAP_FAILED {
            unsafe {
                libc::munmap(large, length);
            }
            anyhow::bail!("Native VM ceiling was not enforced; clipboard was not accessed");
        }
        ensure!(
            std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOMEM),
            "Native VM ceiling could not be verified"
        );
        Ok(value)
    }
    fn limit(&self, growth: u64) -> Result<()> {
        let current = self
            .baseline
            .checked_add(growth)
            .ok_or_else(|| anyhow::anyhow!("Native VM bound overflow"))?
            as libc::rlim_t;
        let limit = libc::rlimit {
            rlim_cur: current.min(self.hard),
            rlim_max: self.hard,
        };
        ensure!(
            unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } == 0,
            "Could not install native clipboard VM bound"
        );
        Ok(())
    }
    pub fn decoding(&mut self) -> Result<()> {
        self.limit(DECODE_GROWTH)
    }
}
#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::*,
        System::{JobObjects::*, Memory::*, Threading::GetCurrentProcess},
    };
    #[repr(C)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak_ws: usize,
        ws: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private_usage: usize,
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(process: HANDLE, counters: *mut Counters, bytes: u32) -> i32;
    }
    pub(crate) struct Budget {
        job: OwnedHandle,
        baseline: u64,
    }
    impl Budget {
        pub fn enter() -> Result<Self> {
            let mut counters: Counters = unsafe { std::mem::zeroed() };
            counters.cb = std::mem::size_of::<Counters>() as u32;
            ensure!(
                unsafe {
                    GetProcessMemoryInfo(
                        GetCurrentProcess(),
                        &mut counters,
                        std::mem::size_of::<Counters>() as u32,
                    )
                } != 0,
                "Native private memory accounting unavailable"
            );
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            ensure!(
                !raw.is_null(),
                "Could not create native clipboard memory boundary"
            );
            let value = Self {
                job: unsafe { OwnedHandle::from_raw_handle(raw) },
                baseline: counters.private_usage as u64,
            };
            value.limit(NATIVE_GROWTH)?;
            ensure!(
                unsafe { AssignProcessToJobObject(value.job.as_raw_handle(), GetCurrentProcess()) }
                    != 0,
                "Could not contain native clipboard allocations"
            );
            let small = unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    4096,
                    MEM_RESERVE | MEM_COMMIT,
                    PAGE_READWRITE,
                )
            };
            ensure!(!small.is_null(), "Native bounded allocation unavailable");
            unsafe {
                VirtualFree(small, 0, MEM_RELEASE);
            }
            let large = unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    (NATIVE_GROWTH + 65536) as usize,
                    MEM_RESERVE | MEM_COMMIT,
                    PAGE_READWRITE,
                )
            };
            if !large.is_null() {
                unsafe {
                    VirtualFree(large, 0, MEM_RELEASE);
                }
                anyhow::bail!(
                    "Native private-memory ceiling was not enforced; clipboard was not accessed"
                );
            }
            Ok(value)
        }
        fn limit(&self, growth: u64) -> Result<()> {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            info.ProcessMemoryLimit = usize::try_from(
                self.baseline
                    .checked_add(growth)
                    .ok_or_else(|| anyhow::anyhow!("Native memory bound overflow"))?,
            )?;
            ensure!(
                unsafe {
                    SetInformationJobObject(
                        self.job.as_raw_handle(),
                        JobObjectExtendedLimitInformation,
                        (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                        std::mem::size_of_val(&info) as u32,
                    )
                } != 0,
                "Could not install native clipboard memory bound"
            );
            Ok(())
        }
        pub fn dib_acquisition(&mut self) -> Result<()> {
            self.limit(crate::MAX_CAPTURE_BYTES + 64 * 1024 + 16 * 1024 * 1024)
        }
        pub fn decoding(&mut self) -> Result<()> {
            self.limit(DECODE_GROWTH)
        }
    }
}
#[cfg(target_os = "windows")]
pub(super) use windows::Budget;
