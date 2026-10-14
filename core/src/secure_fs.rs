//! Writing secrets so only the current user can read them.
//!
//! Private keys and the like are written through [`write_private`]: mode
//! `0600` on Unix, and on Windows a protected DACL with a single entry for the
//! current user (no inherited Administrators/SYSTEM entries). Elsewhere (the
//! Android bridge writes into app-private storage) it is a plain write.

use std::io;
use std::path::Path;

/// Write `bytes` to `path`, creating or truncating it, readable only by the
/// current user.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        // `mode` only applies when the file is created; tighten an old one too.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
        restrict_to_owner(path)
    }
}

/// Create `path` (and its parents) as a directory only the current user can
/// open: mode `0700` on Unix, an owner-only DACL on Windows whose entry the
/// folder's contents inherit, so what it already holds and what is written
/// into it later stay readable by the owner (and by no one else). For
/// folders that other code fills with secrets, such as the relay's Let's
/// Encrypt keys.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        restrict(path, true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(())
    }
}

/// Replace `path`'s DACL with one entry granting the current user full
/// control, and stop inheriting the parent's entries.
#[cfg(windows)]
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    restrict(path, false)
}

/// [`restrict_to_owner`]; with `inherited`, the entry is passed down to the
/// folder's files and subfolders. Without it, protecting a folder would leave
/// every child that had only inherited entries with none at all.
#[cfg(windows)]
fn restrict(path: &Path, inherited: bool) -> io::Result<()> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
        TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, NO_INHERITANCE,
        PROTECTED_DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::{HSTRING, PWSTR};

    let os_err = |e: windows::core::Error| io::Error::from_raw_os_error(e.code().0 & 0xFFFF);

    // The current user's SID, from the process token.
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(os_err)?;
    let mut needed = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    let mut buffer = vec![0u8; needed as usize];
    let filled = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    };
    let _ = unsafe { CloseHandle(token) };
    filled.map_err(os_err)?;
    // SAFETY: GetTokenInformation(TokenUser) filled `buffer` with a TOKEN_USER
    // whose SID points into the same buffer, which outlives every use below.
    let sid = unsafe { (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid };

    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS.0,
        grfAccessMode: SET_ACCESS,
        grfInheritance: if inherited {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: PWSTR(sid.0.cast()),
            ..Default::default()
        },
    };
    let mut acl: *mut ACL = std::ptr::null_mut();
    unsafe { SetEntriesInAclW(Some(&[entry]), None, &mut acl) }
        .ok()
        .map_err(os_err)?;
    let set = unsafe {
        SetNamedSecurityInfoW(
            &HSTRING::from(path.as_os_str()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(acl),
            None,
        )
    };
    unsafe { LocalFree(Some(HLOCAL(acl.cast()))) };
    set.ok().map_err(os_err)
}

#[cfg(not(any(unix, windows)))]
fn restrict_to_owner(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pocketdesk-secure-fs-{}-{label}",
            std::process::id()
        ))
    }

    #[test]
    fn a_private_dir_is_created_and_can_hold_files() {
        let dir = temp("dir").join("nested");
        create_private_dir(&dir).unwrap();
        create_private_dir(&dir).unwrap();
        std::fs::write(dir.join("key.pem"), b"secret").unwrap();
        assert_eq!(std::fs::read(dir.join("key.pem")).unwrap(), b"secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::remove_dir_all(temp("dir")).ok();
    }

    /// Making a folder private must not lock its owner out of what it
    /// already holds. On Windows, protecting a folder's DACL recomputes its
    /// children's inherited entries: with an entry the children do not
    /// inherit, a file that had only inherited ones was left with none, and
    /// nobody could read it (it took the server's trust store with it).
    #[test]
    fn making_a_folder_private_keeps_what_it_holds_readable() {
        let dir = temp("existing");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("before.json"), b"kept").unwrap();
        std::fs::write(dir.join("sub").join("deeper.json"), b"kept too").unwrap();
        create_private_dir(&dir).unwrap();
        assert_eq!(std::fs::read(dir.join("before.json")).unwrap(), b"kept");
        assert_eq!(
            std::fs::read(dir.join("sub").join("deeper.json")).unwrap(),
            b"kept too"
        );
        std::fs::write(dir.join("after.json"), b"new").unwrap();
        assert_eq!(std::fs::read(dir.join("after.json")).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn writes_and_overwrites_the_contents() {
        let path = temp("contents");
        write_private(&path, b"first").unwrap();
        write_private(&path, b"2nd").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"2nd");
        std::fs::remove_file(&path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn unix_mode_is_owner_only_even_for_an_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp("mode");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_file(&path).ok();
    }

    #[cfg(windows)]
    #[test]
    fn windows_dacl_is_protected_with_a_single_entry() {
        use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
        use windows::Win32::Security::{
            ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, PSECURITY_DESCRIPTOR,
            SE_DACL_PROTECTED,
        };
        use windows::core::HSTRING;

        let path = temp("dacl");
        write_private(&path, b"secret").unwrap();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            GetNamedSecurityInfoW(
                &HSTRING::from(path.as_os_str()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut dacl),
                None,
                &mut descriptor,
            )
        }
        .ok()
        .unwrap();
        assert_eq!(unsafe { (*dacl).AceCount }, 1, "exactly one ACE: the owner");
        let mut control = 0u16;
        let mut revision = 0u32;
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }.unwrap();
        assert_ne!(control & SE_DACL_PROTECTED.0, 0, "inheritance is cut");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"secret",
            "owner can still read"
        );
        std::fs::remove_file(&path).ok();
    }
}
