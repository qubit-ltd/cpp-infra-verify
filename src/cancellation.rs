// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Cooperative cancellation with process-tree cleanup.
use std::sync::Once;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
static CANCELLED: AtomicBool = AtomicBool::new(false);
static INSTALLED: Once = Once::new();
extern "C" fn interrupt(_: libc::c_int) {
    CANCELLED.store(true, Ordering::SeqCst);
}
pub(crate) fn install() {
    INSTALLED.call_once(|| {
        // The handler only touches a lock-free atomic and leaves cleanup to normal
        // code.
        unsafe {
            libc::signal(libc::SIGINT, interrupt as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, interrupt as *const () as libc::sighandler_t);
        }
    });
}
pub(crate) fn cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}
pub(crate) fn terminate_tree(pid: u32) {
    #[cfg(target_os = "linux")]
    {
        // Ninja may put jobs in their own process groups, so include descendants.
        if let Ok(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) {
            for child in children
                .split_whitespace()
                .filter_map(|child| child.parse::<u32>().ok())
            {
                terminate_tree(child);
            }
        }
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
        libc::kill(pid as i32, libc::SIGKILL);
    }
}
