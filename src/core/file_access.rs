//! Who can reach a file: its rwx bits, owner and group on Unix, with its ACL on
//! Linux and macOS; its DACL and owner on Windows.
//!
//! A file replaced by renaming a new one over it ends up with the new one's: the
//! creating process's group and whatever ACL the directory hands down, with the
//! temp file's mode. [`FileAccess`] carries the replaced file's over, so the
//! rewrite neither narrows nor widens who can read it.

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

/// Who may reach the file being replaced, to carry onto the new one.
pub struct FileAccess {
    #[cfg(unix)]
    owner: Option<(u32, u32)>,
    /// The group must come back exactly. Not when it carries no access of its
    /// own (see [`group_carries_access`]), nor for a file RTK makes owner-only
    /// because it could not read it, whose group no one but its owner can use.
    #[cfg(unix)]
    exact_group: bool,
    #[cfg(unix)]
    mode: u32,
    #[cfg(target_os = "linux")]
    acl: Option<Vec<u8>>,
    #[cfg(target_os = "macos")]
    acl: Option<macos::Acl>,
    #[cfg(windows)]
    security: Option<windows::Security>,
}

impl FileAccess {
    /// The access of the file at `path` to carry onto the one replacing it, or
    /// `None` when there is no file to replace (missing, or a link that loops).
    /// On Unix a file RTK or its owner cannot read, or that cannot be stat'd,
    /// has no access worth keeping: see [`FileAccess::owner_only`].
    pub fn for_replaced(path: &Path) -> Result<Option<Self>> {
        Self::for_replaced_with(path, fs::metadata(path))
    }

    /// [`FileAccess::for_replaced`] given how stat'ing `path` went.
    fn for_replaced_with(path: &Path, stat: io::Result<fs::Metadata>) -> Result<Option<Self>> {
        let meta = match stat {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            #[cfg(unix)]
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
            Err(_) => return Ok(Some(Self::owner_only(None))),
        };
        #[cfg(unix)]
        if !readable(path, &meta) {
            return Ok(Some(Self::owner_only(Some(&meta))));
        }
        #[cfg(not(unix))]
        let _ = (path, &meta);
        #[cfg(unix)]
        let mode = meta.mode() & 0o777;
        #[cfg(target_os = "linux")]
        let acl = linux::read_acl(path).context("Failed to read the file's ACL")?;
        #[cfg(target_os = "linux")]
        let has_acl = acl.is_some();
        #[cfg(all(unix, not(target_os = "linux")))]
        let has_acl = false;
        Ok(Some(Self {
            #[cfg(unix)]
            owner: Some((meta.uid(), meta.gid())),
            #[cfg(unix)]
            exact_group: group_carries_access(mode, has_acl),
            #[cfg(unix)]
            mode,
            #[cfg(target_os = "linux")]
            acl,
            #[cfg(target_os = "macos")]
            acl: macos::read_acl(path).context("Failed to read the file's ACL")?,
            #[cfg(windows)]
            security: windows::read(path)
                .context("Failed to read the file's security descriptor")?,
        }))
    }

    /// Only the new file's owner may read and write it: 0600 without the ACL its
    /// directory hands down, with the replaced file's owner and group where the
    /// process may set them, so its owner is RTK's user when it cannot. Windows
    /// has no such floor: the new file keeps what its directory hands down.
    fn owner_only(meta: Option<&fs::Metadata>) -> Self {
        #[cfg(not(unix))]
        let _ = meta;
        Self {
            #[cfg(unix)]
            owner: meta.map(|meta| (meta.uid(), meta.gid())),
            #[cfg(unix)]
            exact_group: false,
            #[cfg(unix)]
            mode: 0o600,
            #[cfg(target_os = "linux")]
            acl: None,
            #[cfg(target_os = "macos")]
            acl: None,
            #[cfg(windows)]
            security: None,
        }
    }

    /// The permissions to create the temp file with, before the umask:
    /// owner-only while it replaces a file, so the content is never readable by
    /// more users than that file and a refused chmod leaves it so; a new file's
    /// otherwise.
    #[cfg(unix)]
    pub fn temp_permissions(replacing: bool) -> fs::Permissions {
        fs::Permissions::from_mode(if replacing { 0o600 } else { 0o666 })
    }

    /// Before any content is written, so a file whose access cannot be copied is
    /// refused before any work: the group and owner, then the ACL or DACL, which
    /// replaces what the directory handed down. A Linux ACL carries the mode, so
    /// the file goes back to owner-only until [`FileAccess::after_write`]: that
    /// masks the ACL's group and named entries, and keeps the file writable on a
    /// network filesystem that checks the mode on every write.
    pub fn before_write(&self, temp_path: &Path, temp: &File) -> Result<()> {
        #[cfg(not(windows))]
        let _ = temp_path;
        #[cfg(not(unix))]
        let _ = temp;
        #[cfg(unix)]
        self.copy_owner(temp)?;
        #[cfg(target_os = "linux")]
        {
            linux::write_acl(temp, self.acl.as_deref()).context("Failed to copy the file's ACL")?;
            let _ = temp.set_permissions(fs::Permissions::from_mode(0o600));
        }
        #[cfg(target_os = "macos")]
        macos::write_acl(temp, self.acl.as_ref()).context("Failed to copy the file's ACL")?;
        #[cfg(windows)]
        if let Some(security) = &self.security {
            windows::apply(security, temp_path)
                .context("Failed to copy the file's security descriptor")?;
        }
        Ok(())
    }

    /// After the content is written: the rwx bits, which also put back a Linux
    /// ACL's mask, since a mode's group bits are the mask of the ACL it has.
    pub fn after_write(&self, temp: &File) -> Result<()> {
        #[cfg(not(unix))]
        let _ = temp;
        // Best effort: a filesystem that refuses chmod still gets the content,
        // no wider than the file it replaces.
        #[cfg(unix)]
        let _ = temp.set_permissions(fs::Permissions::from_mode(self.mode));
        Ok(())
    }

    /// The owner where the process may set it (as root), and the group, which
    /// must come back when it carries access. An id the user namespace
    /// does not map, shown as its overflow id, is one the kernel refuses to set.
    #[cfg(unix)]
    fn copy_owner(&self, temp: &File) -> Result<()> {
        use std::os::unix::fs::fchown;
        let Some((uid, gid)) = self.owner else {
            return Ok(());
        };
        let meta = temp.metadata().context("Failed to stat the new file")?;
        // The group first: once the file is given away, RTK may no longer set it.
        if meta.gid() != gid
            && let Err(error) = fchown(temp, None, Some(gid))
            && self.exact_group
        {
            return Err(error).with_context(|| {
                format!("Failed to give the new file the replaced file's group {gid}")
            });
        }
        if meta.uid() != uid {
            let _ = fchown(temp, Some(uid), None);
        }
        Ok(())
    }
}

/// Whether the owning group's members get other access than everyone else:
/// through rwx bits that differ from the other bits (fewer deny them what others
/// get), or through a Linux ACL, whose group entries are not in the mode. A group
/// that carries no access of its own may be the new file's process group: a file
/// root left 0644 is rewritten 0644 as RTK's user's, and no one gains access.
#[cfg(unix)]
pub(crate) fn group_carries_access(mode: u32, has_acl: bool) -> bool {
    has_acl || (mode >> 3) & 0o7 != mode & 0o7
}

/// Whether RTK may read `path`, and its owner may too.
#[cfg(unix)]
fn readable(path: &Path, meta: &fs::Metadata) -> bool {
    use std::os::unix::ffi::OsStrExt;
    if meta.permissions().mode() & 0o400 == 0 {
        return false;
    }
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    #[allow(unsafe_code)]
    // nosemgrep: unsafe-block — access() only checks the NUL-terminated path
    unsafe {
        libc::access(path.as_ptr(), libc::R_OK) == 0
    }
}

/// The Linux ACL wire format, for tests that build ACLs without `setfacl`.
#[cfg(all(test, target_os = "linux"))]
pub(crate) mod acl_wire {
    pub(crate) use super::linux::{ACCESS_ACL, read_acl};

    pub(crate) const USER_OBJ: u16 = 0x01;
    pub(crate) const USER: u16 = 0x02;
    pub(crate) const GROUP_OBJ: u16 = 0x04;
    pub(crate) const MASK: u16 = 0x10;
    pub(crate) const OTHER: u16 = 0x20;

    pub(crate) fn encode(entries: &[(u16, u16, u32)]) -> Vec<u8> {
        let mut acl = 2u32.to_le_bytes().to_vec();
        for &(tag, perms, id) in entries {
            acl.extend(tag.to_le_bytes());
            acl.extend(perms.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        acl
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub(crate) const ACCESS_ACL: &CStr = c"system.posix_acl_access";

    /// The access ACL as the kernel stores it, or `None` when the file has none
    /// or its filesystem has no ACLs.
    pub(crate) fn read_acl(path: &Path) -> io::Result<Option<Vec<u8>>> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        loop {
            #[allow(unsafe_code)]
            // nosemgrep: unsafe-block — getxattr with a null buffer only reports the size
            let size = unsafe {
                libc::getxattr(path.as_ptr(), ACCESS_ACL.as_ptr(), std::ptr::null_mut(), 0)
            };
            if size < 0 {
                return absent(io::Error::last_os_error());
            }
            // No access ACL is empty: an empty value is none.
            if size == 0 {
                return Ok(None);
            }
            let mut acl = vec![0u8; size as usize];
            #[allow(unsafe_code)]
            // nosemgrep: unsafe-block — the buffer is `acl.len()` bytes long
            let read = unsafe {
                libc::getxattr(
                    path.as_ptr(),
                    ACCESS_ACL.as_ptr(),
                    acl.as_mut_ptr().cast(),
                    acl.len(),
                )
            };
            if read >= 0 {
                acl.truncate(read as usize);
                return Ok(Some(acl));
            }
            let error = io::Error::last_os_error();
            // The ACL grew between the two calls: ask for its size again.
            if error.raw_os_error() != Some(libc::ERANGE) {
                return absent(error);
            }
        }
    }

    fn absent(error: io::Error) -> io::Result<Option<Vec<u8>>> {
        match error.raw_os_error() {
            Some(libc::ENODATA | libc::EOPNOTSUPP) => Ok(None),
            _ => Err(error),
        }
    }

    /// Give `file` the access ACL `acl`, or none: an ACL its directory handed
    /// down is removed.
    pub(super) fn write_acl(file: &File, acl: Option<&[u8]>) -> io::Result<()> {
        match acl {
            Some(acl) => set_acl(file, Some(acl)),
            None if has_acl(file)? => set_acl(file, None),
            None => Ok(()),
        }
    }

    /// Whether `file` has an access ACL, as one its directory handed down.
    fn has_acl(file: &File) -> io::Result<bool> {
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — fgetxattr with a null buffer only reports the size
        let size = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                ACCESS_ACL.as_ptr(),
                std::ptr::null_mut(),
                0,
            )
        };
        if size >= 0 {
            // No access ACL is empty: an empty value is none, as in `read_acl`.
            return Ok(size > 0);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENODATA | libc::EOPNOTSUPP) => Ok(false),
            _ => Err(error),
        }
    }

    fn set_acl(file: &File, acl: Option<&[u8]>) -> io::Result<()> {
        let fd = file.as_raw_fd();
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — `fd` is open for the duration, `acl` is a live slice
        let result = unsafe {
            match acl {
                Some(acl) => {
                    libc::fsetxattr(fd, ACCESS_ACL.as_ptr(), acl.as_ptr().cast(), acl.len(), 0)
                }
                None => libc::fremovexattr(fd, ACCESS_ACL.as_ptr()),
            }
        };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match (acl, error.raw_os_error()) {
            (None, Some(libc::ENODATA | libc::EOPNOTSUPP)) => Ok(()),
            _ => Err(error),
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::fs::File;
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;

    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn acl_get_file(path: *const c_char, kind: c_int) -> *mut c_void;
        fn acl_get_fd_np(fd: c_int, kind: c_int) -> *mut c_void;
        fn acl_set_fd_np(fd: c_int, acl: *mut c_void, kind: c_int) -> c_int;
        fn acl_init(count: c_int) -> *mut c_void;
        fn acl_free(object: *mut c_void) -> c_int;
    }

    /// An ACL allocated by the system library, freed on drop.
    pub(super) struct Acl(*mut c_void);

    impl Drop for Acl {
        fn drop(&mut self) {
            #[allow(unsafe_code)]
            // nosemgrep: unsafe-block — the pointer came from acl_get_file or acl_init
            unsafe {
                acl_free(self.0);
            }
        }
    }

    /// Whether `file` has an extended ACL.
    fn has_acl(file: &File) -> io::Result<bool> {
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — the fd is open for the duration
        let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
        if !acl.is_null() {
            drop(Acl(acl));
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTSUP | libc::EOPNOTSUPP) => Ok(false),
            _ => Err(error),
        }
    }

    /// The extended ACL, or `None` when the file has none or its filesystem has
    /// no ACLs.
    pub(super) fn read_acl(path: &Path) -> io::Result<Option<Acl>> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — `path` is NUL-terminated
        let acl = unsafe { acl_get_file(path.as_ptr(), ACL_TYPE_EXTENDED) };
        if !acl.is_null() {
            return Ok(Some(Acl(acl)));
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTSUP | libc::EOPNOTSUPP) => Ok(None),
            _ => Err(error),
        }
    }

    /// Give `file` the extended ACL `acl`, or none when it has one its directory
    /// handed down.
    pub(super) fn write_acl(file: &File, acl: Option<&Acl>) -> io::Result<()> {
        let clearing = acl.is_none();
        if clearing && !has_acl(file)? {
            return Ok(());
        }
        let empty;
        let acl = match acl {
            Some(acl) => acl,
            None => {
                #[allow(unsafe_code)]
                // nosemgrep: unsafe-block — acl_init allocates or returns null
                let raw = unsafe { acl_init(0) };
                if raw.is_null() {
                    return Err(io::Error::last_os_error());
                }
                empty = Acl(raw);
                &empty
            }
        };
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — the fd is open and the ACL is live
        let result = unsafe { acl_set_fd_np(file.as_raw_fd(), acl.0, ACL_TYPE_EXTENDED) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOTSUP | libc::EOPNOTSUPP) if clearing => Ok(()),
            _ => Err(error),
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::fs;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, OBJECT_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_AUTO_INHERIT_REQ,
        SE_DACL_AUTO_INHERITED, SE_DACL_PROTECTED, SetKernelObjectSecurity,
        SetSecurityDescriptorControl,
    };

    /// Standard access rights (winnt.h), named here rather than pulling in
    /// windows-sys's file-system feature for them.
    const WRITE_DAC: u32 = 0x0004_0000;
    const WRITE_OWNER: u32 = 0x0008_0000;
    /// FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE: the temp file is
    /// already open for writing.
    const FILE_SHARE_ALL: u32 = 0x7;

    /// A file's owner and DACL in a descriptor freed on drop, with its DACL
    /// pointing into it.
    pub(super) struct Security {
        descriptor: PSECURITY_DESCRIPTOR,
        dacl: *mut ACL,
        protected: bool,
        auto_inherited: bool,
    }

    impl Drop for Security {
        fn drop(&mut self) {
            #[allow(unsafe_code)]
            // nosemgrep: unsafe-block — the descriptor came from GetNamedSecurityInfoW
            unsafe {
                LocalFree(self.descriptor);
            }
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// ERROR_INVALID_FUNCTION and ERROR_NOT_SUPPORTED: a volume without NT
    /// security, such as FAT or a network redirector.
    fn no_security(status: u32) -> bool {
        matches!(status, 1 | 50)
    }

    /// The file's owner and DACL, or `None` on a volume without NT security.
    pub(super) fn read(path: &Path) -> io::Result<Option<Security>> {
        let path = wide(path);
        let mut security = Security {
            descriptor: std::ptr::null_mut(),
            dacl: std::ptr::null_mut(),
            protected: false,
            auto_inherited: false,
        };
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — every out-pointer is a live local
        let status = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut security.dacl,
                std::ptr::null_mut(),
                &mut security.descriptor,
            )
        };
        if no_security(status) {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let mut control = 0u16;
        let mut revision = 0u32;
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — the descriptor is live, the out-pointers are locals
        let read = unsafe {
            GetSecurityDescriptorControl(security.descriptor, &mut control, &mut revision)
        };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        security.protected = control & SE_DACL_PROTECTED != 0;
        security.auto_inherited = control & SE_DACL_AUTO_INHERITED != 0;
        Ok(Some(security))
    }

    /// Give the file at `path` the DACL in `security` as it stands, with its
    /// protection and the entries it inherited, and its owner where the process
    /// may set it: a different owner only changes what the owner itself may do,
    /// and taking ownership needs a privilege the user rarely holds.
    ///
    /// The descriptor is set on a handle (`SetKernelObjectSecurity`) because
    /// `SetNamedSecurityInfoW` replaces an unprotected DACL's inherited entries
    /// with those of the new file's directory: a file moved in from a stricter
    /// directory would gain what this one grants.
    pub(super) fn apply(security: &Security, path: &Path) -> io::Result<()> {
        // One handle for every pass: its rights are checked when it opens, so a
        // DACL that takes WRITE_DAC away from RTK's user does not stop the next.
        let open = |access| {
            fs::OpenOptions::new()
                .access_mode(access)
                .share_mode(FILE_SHARE_ALL)
                .open(path)
        };
        let copied = open(WRITE_DAC | WRITE_OWNER)
            .or_else(|_| open(WRITE_DAC))
            .and_then(|file| {
                let descriptor = security.descriptor;
                set(
                    &file,
                    descriptor,
                    DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
                )
                .or_else(|_| set(&file, descriptor, DACL_SECURITY_INFORMATION))?;
                if security.auto_inherited {
                    restore_auto_inherited(&file, descriptor)?;
                }
                Ok(())
            });
        // A volume that refuses WRITE_DAC, such as an SMB share granting Change,
        // or one that builds DACLs it cannot set, is no obstacle when the new
        // file already has the DACL, as it does when the replaced one only
        // inherits from the directory.
        copied.or_else(|error| match read(path) {
            Ok(Some(temp)) if same_dacl(&temp, security) => Ok(()),
            _ => Err(error),
        })
    }

    fn set(
        file: &fs::File,
        descriptor: PSECURITY_DESCRIPTOR,
        info: OBJECT_SECURITY_INFORMATION,
    ) -> io::Result<()> {
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — the handle is open, the descriptor live
        let done = unsafe { SetKernelObjectSecurity(file.as_raw_handle(), info, descriptor) };
        if done == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Set the file's DACL again with the auto-inherited flag. The kernel keeps
    /// that flag only when asked for automatic inheritance, which keeps the
    /// entries the file already inherited instead of the given ones: the first
    /// pass made those the original's, so this one only restores the flag.
    fn restore_auto_inherited(file: &fs::File, descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
        let request = |bits| {
            #[allow(unsafe_code)]
            // nosemgrep: unsafe-block — the descriptor is live
            let done = unsafe {
                // SE_DACL_AUTO_INHERIT_REQ alone is set or cleared.
                SetSecurityDescriptorControl(descriptor, SE_DACL_AUTO_INHERIT_REQ, bits)
            };
            if done == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        };
        request(SE_DACL_AUTO_INHERIT_REQ)?;
        let result = set(file, descriptor, DACL_SECURITY_INFORMATION);
        // The descriptor read from the original goes back as it was.
        request(0).and(result)
    }

    /// Whether two descriptors grant the same: same protection and the same
    /// ACEs, whatever spare room either ACL buffer has.
    pub(super) fn same_dacl(a: &Security, b: &Security) -> bool {
        match (aces(a.dacl), aces(b.dacl)) {
            (Some(a_aces), Some(b_aces)) => a.protected == b.protected && a_aces == b_aces,
            _ => false,
        }
    }

    /// The ACE count and the bytes of the ACEs of `acl`: `Some(None)` for a null
    /// DACL, which grants everyone everything, and `None` for an ACL whose ACEs
    /// do not fit in it, which is never the same as another.
    fn aces(acl: *mut ACL) -> Option<Option<(u16, Vec<u8>)>> {
        if acl.is_null() {
            return Some(None);
        }
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — the ACL is `AclSize` bytes inside a live descriptor
        let (count, bytes) = unsafe {
            let size = usize::from((*acl).AclSize);
            (
                (*acl).AceCount,
                std::slice::from_raw_parts(acl.cast::<u8>(), size),
            )
        };
        // Each ACE starts with its type, flags and size, after the 8-byte ACL
        // header.
        let mut end = 8;
        for _ in 0..count {
            let size = bytes.get(end + 2..end + 4)?;
            end += usize::from(u16::from_le_bytes([size[0], size[1]]));
        }
        Some(Some((count, bytes.get(8..end)?.to_vec())))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::FileAccess;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    #[cfg(unix)]
    use std::path::Path;

    #[cfg(unix)]
    fn access(uid: u32, gid: u32) -> FileAccess {
        FileAccess {
            owner: Some((uid, gid)),
            exact_group: true,
            mode: 0o640,
            #[cfg(target_os = "linux")]
            acl: None,
            #[cfg(target_os = "macos")]
            acl: None,
        }
    }

    /// The process's groups, as `id -G` lists them; `None` when running as root,
    /// for whom every owner and group change succeeds.
    #[cfg(unix)]
    fn unprivileged_groups() -> Option<Vec<u32>> {
        let id = crate::core::test_isolation::id;
        let (Some(uid), Some(groups)) = (id("-u"), id("-G")) else {
            eprintln!("skipping: `id` cannot list the process's groups");
            return None;
        };
        if uid.trim() == "0" {
            eprintln!("skipping: root may set any owner and group");
            return None;
        }
        Some(
            groups
                .split_whitespace()
                .filter_map(|gid| gid.parse().ok())
                .collect(),
        )
    }

    /// Whether the kernel refuses this change on a scratch file, as it does for
    /// a process without CAP_CHOWN; false, with a notice, when it allows it.
    #[cfg(unix)]
    fn chown_refused(dir: &Path, uid: Option<u32>, gid: Option<u32>) -> bool {
        let probe = std::fs::File::create(dir.join("probe")).expect("create probe");
        let refused = std::os::unix::fs::fchown(&probe, uid, gid).is_err();
        if !refused {
            eprintln!("skipping: the process may set this owner or group (CAP_CHOWN?)");
        }
        refused
    }

    #[cfg(unix)]
    fn temp_file(dir: &Path) -> std::fs::File {
        std::fs::File::create(dir.join("new")).expect("create file")
    }

    /// A gid neither the process nor `file` belongs to: changing to the file's
    /// own group is always allowed.
    #[cfg(unix)]
    fn foreign_gid(groups: &[u32], file: &std::fs::File) -> Option<u32> {
        let own = file.metadata().expect("stat file").gid();
        (1..1000).find(|gid| !groups.contains(gid) && *gid != own)
    }

    #[cfg(unix)]
    #[test]
    fn test_an_owner_rtk_cannot_set_still_gets_the_group() {
        let Some(groups) = unprivileged_groups() else {
            return;
        };
        let dir = tempfile::tempdir().expect("create temp dir");
        let file = temp_file(dir.path());
        let own_gid = file.metadata().expect("stat file").gid();
        let Some(gid) = groups.into_iter().find(|&gid| gid != own_gid) else {
            eprintln!("skipping: the process belongs to no second group");
            return;
        };
        if !chown_refused(dir.path(), Some(0), None) {
            return;
        }
        let probe = std::fs::File::create(dir.path().join("group-probe")).expect("create probe");
        if let Err(error) = std::os::unix::fs::fchown(&probe, None, Some(gid)) {
            eprintln!("skipping: the process cannot give a file its group {gid}: {error}");
            return;
        }

        access(0, gid).copy_owner(&file).expect("keep the group");

        assert_eq!(file.metadata().expect("stat file").gid(), gid);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_group_rtk_cannot_set_fails_the_write_unless_owner_only() {
        let Some(groups) = unprivileged_groups() else {
            return;
        };
        let dir = tempfile::tempdir().expect("create temp dir");
        let file = temp_file(dir.path());
        let Some(foreign) = foreign_gid(&groups, &file) else {
            eprintln!("skipping: no gid below 1000 is foreign to the process");
            return;
        };
        if !chown_refused(dir.path(), None, Some(foreign)) {
            return;
        }
        let uid = file.metadata().expect("stat file").uid();

        assert!(access(uid, foreign).copy_owner(&file).is_err());
        let owner_only = FileAccess {
            exact_group: false,
            ..access(uid, foreign)
        };
        owner_only
            .copy_owner(&file)
            .expect("an owner-only file needs no group");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_an_acl_the_kernel_refuses_fails_the_write() {
        use super::acl_wire::*;

        let dir = tempfile::tempdir().expect("create temp dir");
        let file = temp_file(dir.path());
        let meta = file.metadata().expect("stat file");
        let plain = encode(&[
            (USER_OBJ, 6, u32::MAX),
            (GROUP_OBJ, 0, u32::MAX),
            (OTHER, 0, u32::MAX),
        ]);
        if let Err(error) = super::linux::write_acl(&file, Some(&plain)) {
            eprintln!("skipping: ACLs cannot be set here: {error}");
            return;
        }
        // A named user without a mask entry: the kernel refuses it with EINVAL,
        // as it does an id the user namespace does not map.
        let acl = encode(&[
            (USER_OBJ, 6, u32::MAX),
            (USER, 6, meta.uid()),
            (GROUP_OBJ, 0, u32::MAX),
            (OTHER, 0, u32::MAX),
        ]);
        let access = FileAccess {
            acl: Some(acl),
            ..access(meta.uid(), meta.gid())
        };

        assert!(access.before_write(&dir.path().join("new"), &file).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn test_same_dacl_tells_an_inherited_dacl_from_a_changed_one() {
        use super::windows::{read, same_dacl};

        let dir = tempfile::tempdir().expect("create temp dir");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, "").expect("seed a");
        std::fs::write(&b, "").expect("seed b");
        let security = |path: &std::path::Path| {
            read(path)
                .expect("read security")
                .expect("an NTFS temp dir has NT security")
        };
        assert!(same_dacl(&security(&a), &security(&b)), "both only inherit");

        // Read access for the built-in Guests group, by SID so the name's
        // language does not matter.
        let status = std::process::Command::new("icacls")
            .arg(&b)
            .args(["/grant", "*S-1-5-32-546:(R)", "/Q"])
            .status()
            .expect("run icacls");
        assert!(status.success(), "icacls /grant failed");

        assert!(!same_dacl(&security(&a), &security(&b)));
    }

    #[cfg(unix)]
    #[test]
    fn test_a_temp_file_replacing_one_starts_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(FileAccess::temp_permissions(false).mode(), 0o666);
        assert_eq!(FileAccess::temp_permissions(true).mode(), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_file_rtk_cannot_stat_is_left_owner_only() {
        use std::io::{Error, ErrorKind};

        let path = Path::new("settings.json");
        assert!(
            FileAccess::for_replaced_with(path, Err(Error::from(ErrorKind::NotFound)))
                .expect("a missing file")
                .is_none()
        );
        let access =
            FileAccess::for_replaced_with(path, Err(Error::from(ErrorKind::PermissionDenied)))
                .expect("an unstat-able file")
                .expect("access to apply");
        assert_eq!(access.mode, 0o600);
        assert!(access.owner.is_none());
        assert!(!access.exact_group);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_replaced_file_needs_its_group_when_it_carries_access() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}").expect("seed file");
        let exact_group = |mode: u32| {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
            #[cfg(target_os = "linux")]
            super::linux::write_acl(&std::fs::File::open(&path).expect("open"), None)
                .expect("drop an inherited ACL");
            let access = FileAccess::for_replaced(&path)
                .expect("read access")
                .expect("an existing file");
            (access.mode == mode).then_some(access.exact_group)
        };
        for (mode, exact) in [(0o644, false), (0o640, true), (0o604, true)] {
            let Some(got) = exact_group(mode) else {
                eprintln!("skipping: this filesystem does not keep mode {mode:o}");
                return;
            };
            assert_eq!(got, exact, "mode {mode:o}");
        }

        #[cfg(target_os = "linux")]
        {
            use super::acl_wire::*;

            let _ = exact_group(0o644);
            let uid = std::fs::metadata(&path).expect("stat").uid();
            let acl = encode(&[
                (USER_OBJ, 6, u32::MAX),
                (USER, 4, uid),
                (GROUP_OBJ, 4, u32::MAX),
                (MASK, 4, u32::MAX),
                (OTHER, 4, u32::MAX),
            ]);
            let file = std::fs::File::open(&path).expect("open");
            if let Err(error) = super::linux::write_acl(&file, Some(&acl)) {
                eprintln!("skipping: ACLs cannot be set here: {error}");
                return;
            }
            let access = FileAccess::for_replaced(&path)
                .expect("read access")
                .expect("an existing file");
            assert!(access.exact_group, "a file with an ACL keeps its group");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_a_group_is_kept_only_when_it_carries_access() {
        // Group bits other than the other bits carry access, fewer ones too: they
        // deny the group's members what others get.
        for (mode, carries) in [
            (0o644, false),
            (0o600, false),
            (0o666, false),
            (0o755, false),
            (0o645, true),
            (0o604, true),
            (0o640, true),
            (0o654, true),
            (0o664, true),
            (0o750, true),
            (0o751, true),
        ] {
            assert_eq!(
                super::group_carries_access(mode, false),
                carries,
                "mode {mode:o}"
            );
            assert!(
                super::group_carries_access(mode, true),
                "mode {mode:o} with an ACL"
            );
        }
    }
}
