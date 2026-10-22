//! Windows executables of this package reserve an 8 MiB main-thread stack (see
//! `build.rs`). The test executables get it from the same link argument as
//! rtk.exe, so the running one is read back.
//!
//! build.rs spells the request per linker (`/STACK:` for msvc, `-Wl,--stack,`
//! otherwise). A linker that does not know the spelling either fails the link or
//! drops the request, and only the image shows which.

// 64-bit images only: they are PE32+, the one header layout this reads.
#![cfg(all(windows, target_pointer_width = "64"))]

use std::fs::File;
use std::io::Read;

const STACK_RESERVE: u64 = 8 * 1024 * 1024;

/// The PE headers sit at the start of the image; reading them is enough.
const HEADER_BYTES: u64 = 4096;

/// `SizeOfStackReserve` from the optional header of a PE32+ image (x86_64, ARM64).
fn stack_reserve(headers: &[u8]) -> u64 {
    let at = |offset: usize, len: usize| -> &[u8] {
        headers
            .get(offset..offset + len)
            .unwrap_or_else(|| panic!("PE headers end before offset {}", offset + len))
    };
    let nt = u32::from_le_bytes(at(0x3c, 4).try_into().expect("4 bytes")) as usize;
    assert_eq!(at(nt, 4), b"PE\0\0", "not a PE image");
    let optional = nt + 24;
    let magic = u16::from_le_bytes(at(optional, 2).try_into().expect("2 bytes"));
    assert_eq!(magic, 0x20b, "not a PE32+ image");
    u64::from_le_bytes(at(optional + 72, 8).try_into().expect("8 bytes"))
}

#[test]
fn test_exe_reserves_8_mib_of_stack() {
    let exe = std::env::current_exe().expect("path of the running test");
    let mut headers = Vec::new();
    File::open(&exe)
        .expect("open the running test")
        .take(HEADER_BYTES)
        .read_to_end(&mut headers)
        .expect("read the PE headers");
    assert_eq!(stack_reserve(&headers), STACK_RESERVE, "{}", exe.display());
}
