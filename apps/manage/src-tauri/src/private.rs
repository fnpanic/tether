//! Files only the current user may open: the server private key saved for the relays
//! ("Save key files…", docs/key-rotation.md). Windows: created with a protected DACL that grants
//! the current user's SID full access and nothing else (no inherited entries); unix: mode 0600.
//! The permissions are set when the file is created, so no other process can open it in between.

use std::path::Path;

/// Write `data` to a new file at `path` that only the current user can access. An existing file
/// is deleted first (its permissions would otherwise stay).
pub fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", path.display())),
        _ => {}
    }
    use std::io::Write;
    let mut f = create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(data).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn create(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(windows)]
mod win {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn take_pwstr(p: *mut u16) -> String {
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        LocalFree(p.cast());
        s
    }

    /// `S-1-5-21-…` of the user this process runs as.
    pub fn current_user_sid() -> std::io::Result<String> {
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
            // u64 storage: TOKEN_USER needs pointer alignment.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut s = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(take_pwstr(s))
        }
    }

    /// Protected DACL (`P`: no inherited entries) with one entry: full access for the user.
    fn sddl() -> std::io::Result<String> {
        Ok(format!("D:P(A;;FA;;;{})", current_user_sid()?))
    }

    pub fn create(path: &std::path::Path) -> std::io::Result<std::fs::File> {
        use std::os::windows::io::FromRawHandle;
        let sddl = wide(&sddl()?);
        let name = wide(&path.to_string_lossy());
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
            let h = CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, &sa, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, null_mut());
            let err = std::io::Error::last_os_error();
            LocalFree(sd);
            if h == INVALID_HANDLE_VALUE {
                return Err(err);
            }
            Ok(std::fs::File::from_raw_handle(h))
        }
    }

    /// The file's DACL as SDDL (tests read the permissions back with it).
    pub fn dacl_sddl(path: &std::path::Path) -> std::io::Result<String> {
        let name = wide(&path.to_string_lossy());
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            let r = GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), null_mut(), null_mut(), &mut sd);
            if r != 0 {
                return Err(std::io::Error::from_raw_os_error(r as i32));
            }
            let mut s = null_mut();
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut s, null_mut());
            LocalFree(sd);
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(take_pwstr(s))
        }
    }
}

#[cfg(windows)]
use win::create;

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_current_user_can_open_the_file() {
        let dir = std::env::temp_dir().join(format!("tether-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("id_ed25519");
        // A file that already exists with the folder's (inherited) permissions is replaced.
        std::fs::write(&p, b"old").unwrap();
        super::write(&p, b"secret").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"secret");
        #[cfg(windows)]
        {
            let sid = super::win::current_user_sid().unwrap();
            assert!(sid.starts_with("S-1-"), "{sid}");
            // Exactly one entry, no inheritance: what the file's ACL reads back as.
            // SDDL prints the built-in Administrator (RID 500, the CI runner's account) as "LA".
            let who = if sid.ends_with("-500") { "LA".to_string() } else { sid };
            assert_eq!(super::win::dacl_sddl(&p).unwrap(), format!("D:P(A;;FA;;;{who})"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
