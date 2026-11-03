//! The credential file's owner-only DACL on Windows, read and written with Windows' own calls: a moment's work, where
//! each PowerShell run takes 150 ms on a quiet machine and seconds on the first one after a boot.
//!
//! Only Kumi's own shape is judged here: this account owns the file, and its protected DACL holds one explicit allow
//! ACE, for this account, with full control (what `SECURE_FILE` in store.rs writes). A file of that shape is one the
//! PowerShell check (`ACL_CHECKS`) passes too. Any other shape goes to that check, as before: .NET canonicalizes the ACEs
//! it counts (it merges and drops some), so only it can say no.
use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::Path, ptr::null_mut};

type Handle = *mut c_void;
type Sid = *mut c_void;
type Acl = *mut c_void;

#[repr(C)]
struct AclSizeInformation {
    ace_count: u32,
    acl_bytes_in_use: u32,
    acl_bytes_free: u32,
}
#[repr(C)]
struct AceHeader {
    ace_type: u8,
    ace_flags: u8,
    ace_size: u16,
}
/// ACCESS_ALLOWED_ACE and ACCESS_DENIED_ACE: the header, the mask, then the SID, which starts at `sid_start`.
#[repr(C)]
struct MaskAce {
    header: AceHeader,
    mask: u32,
    sid_start: u32,
}
#[repr(C)]
struct SidAndAttributes {
    sid: Sid,
    attributes: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetDriveTypeW(root: *const u16) -> u32;
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(token: Handle, class: u32, information: *mut c_void, length: u32, returned: *mut u32) -> i32;
    fn GetNamedSecurityInfoW(
        name: *const u16,
        kind: u32,
        information: u32,
        owner: *mut Sid,
        group: *mut Sid,
        dacl: *mut Acl,
        sacl: *mut Acl,
        descriptor: *mut *mut c_void,
    ) -> u32;
    fn SetNamedSecurityInfoW(name: *const u16, kind: u32, information: u32, owner: Sid, group: Sid, dacl: Acl, sacl: Acl) -> u32;
    fn GetSecurityDescriptorControl(descriptor: *mut c_void, control: *mut u16, revision: *mut u32) -> i32;
    fn GetAclInformation(acl: Acl, information: *mut c_void, length: u32, class: u32) -> i32;
    fn GetAce(acl: Acl, index: u32, ace: *mut *mut c_void) -> i32;
    fn EqualSid(left: Sid, right: Sid) -> i32;
    fn GetLengthSid(sid: Sid) -> u32;
    fn InitializeAcl(acl: Acl, length: u32, revision: u32) -> i32;
    fn AddAccessAllowedAceEx(acl: Acl, revision: u32, flags: u32, mask: u32, sid: Sid) -> i32;
    #[cfg(test)]
    fn AddAccessDeniedAceEx(acl: Acl, revision: u32, flags: u32, mask: u32, sid: Sid) -> i32;
    #[cfg(test)]
    fn ConvertStringSidToSidW(text: *const u16, sid: *mut Sid) -> i32;
    #[cfg(test)]
    fn InitializeSecurityDescriptor(descriptor: *mut c_void, revision: u32) -> i32;
    #[cfg(test)]
    fn SetSecurityDescriptorDacl(descriptor: *mut c_void, present: i32, dacl: Acl, defaulted: i32) -> i32;
    #[cfg(test)]
    fn SetSecurityDescriptorControl(descriptor: *mut c_void, mask: u16, bits: u16) -> i32;
    #[cfg(test)]
    fn SetFileSecurityW(name: *const u16, information: u32, descriptor: *mut c_void) -> i32;
}

const DRIVE_REMOTE: u32 = 4;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const SE_FILE_OBJECT: u32 = 1;
const OWNER_SECURITY_INFORMATION: u32 = 0x1;
const DACL_SECURITY_INFORMATION: u32 = 0x4;
const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
#[cfg(test)]
const UNPROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x2000_0000;
const SE_DACL_PROTECTED: u16 = 0x1000;
const ACL_SIZE_INFORMATION: u32 = 2;
const ACL_REVISION: u32 = 2;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const TOKEN_QUERY: u32 = 0x8;
const TOKEN_USER: u32 = 1;
/// FileSystemRights.FullControl, what `SECURE_FILE` grants and `ACL_CHECKS` asks for.
const FULL_CONTROL: u32 = 0x001F_01FF;

/// This process's account (TOKEN_USER, whose SID points into the same buffer).
struct Account(Vec<u64>);

impl Account {
    fn current() -> Option<Self> {
        // SAFETY: the token is checked before use and closed once; the second call writes at most `needed` bytes into a
        // buffer at least that long, aligned for TOKEN_USER's pointer.
        unsafe {
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let mut needed = 0u32;
            GetTokenInformation(token, TOKEN_USER, null_mut(), 0, &mut needed);
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
            let read = GetTokenInformation(token, TOKEN_USER, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut needed);
            CloseHandle(token);
            (read != 0).then_some(Self(buffer))
        }
    }
    fn sid(&self) -> Sid {
        // SAFETY: the buffer holds the TOKEN_USER the token gave.
        unsafe { (*self.0.as_ptr().cast::<SidAndAttributes>()).sid }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// Whether the file is on this computer's own disk by a plain path: a drive letter that isn't a share's, and no link or
/// junction on the way to it, the file's own included. Any other path is left to the PowerShell check.
fn plain_local(path: &Path) -> bool {
    use std::{
        os::windows::fs::MetadataExt,
        path::{Component, Prefix},
    };
    let letter = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
            _ => return false,
        },
        _ => return false,
    };
    let root: Vec<u16> = format!("{}:\\", letter as char).encode_utf16().chain([0]).collect();
    // SAFETY: the root is NUL-terminated.
    if unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE {
        return false;
    }
    let mut at = Some(path);
    while let Some(here) = at.filter(|here| here.parent().is_some()) {
        match std::fs::symlink_metadata(here) {
            Ok(meta) if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 => at = here.parent(),
            _ => return false,
        }
    }
    true
}

/// Whether the file is Kumi's own owner-only shape. False whatever the reason it isn't, and also when Windows won't say
/// or the path isn't a plain local one: the PowerShell check then has the last word.
pub fn is_kumis_shape(path: &Path) -> bool {
    if !plain_local(path) {
        return false;
    }
    let Some(account) = Account::current() else { return false };
    let name = wide(path);
    let (mut owner, mut dacl, mut descriptor) = (null_mut(), null_mut(), null_mut());
    // SAFETY: the name is NUL-terminated; on success the descriptor is Windows' own allocation, which the owner and the
    // DACL point into, read before it's freed once.
    unsafe {
        let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        if GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, information, &mut owner, null_mut(), &mut dacl, null_mut(), &mut descriptor)
            != 0
        {
            return false;
        }
        let shaped = shaped(account.sid(), owner, dacl, descriptor);
        LocalFree(descriptor);
        shaped
    }
}

/// Kumi's shape, as `ACL_CHECKS` reads it: owned by `account`; the DACL protected (AreAccessRulesProtected); one ACE
/// (Access.Count), for `account`, not inherited and not inheritable (no flags at all), an allow ACE, and full control
/// exactly: what SECURE_FILE writes, and nothing more.
///
/// # Safety
/// `owner`, `dacl` and `descriptor` come from one GetNamedSecurityInfoW that succeeded, and are still allocated.
unsafe fn shaped(account: Sid, owner: Sid, dacl: Acl, descriptor: *mut c_void) -> bool {
    unsafe {
        if owner.is_null() || EqualSid(owner, account) == 0 {
            return false;
        }
        let (mut control, mut revision) = (0u16, 0u32);
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0 || control & SE_DACL_PROTECTED == 0 {
            return false;
        }
        // A null DACL lets everyone in.
        if dacl.is_null() {
            return false;
        }
        let mut size = AclSizeInformation { ace_count: 0, acl_bytes_in_use: 0, acl_bytes_free: 0 };
        let length = std::mem::size_of::<AclSizeInformation>() as u32;
        if GetAclInformation(dacl, (&raw mut size).cast(), length, ACL_SIZE_INFORMATION) == 0 || size.ace_count != 1 {
            return false;
        }
        let mut ace = null_mut();
        if GetAce(dacl, 0, &mut ace) == 0 || ace.is_null() {
            return false;
        }
        let header = &*ace.cast::<AceHeader>();
        if header.ace_type != ACCESS_ALLOWED_ACE_TYPE || header.ace_flags != 0 {
            return false;
        }
        let ace = ace.cast::<MaskAce>();
        (*ace).mask == FULL_CONTROL && EqualSid((&raw mut (*ace).sid_start).cast(), account) != 0
    }
}

/// Gives the file Kumi's shape, as `SECURE_FILE` does: a protected DACL whose one ACE gives this account full control,
/// then this account as its owner where it isn't. Then it's checked. Err when a step fails, and PowerShell's own step
/// runs instead.
pub fn make_kumis_shape(path: &Path) -> Result<(), String> {
    let account = Account::current().ok_or("this account couldn't be read")?;
    let sid = account.sid();
    let name = wide(path);
    // SAFETY: the ACL is built in a buffer of the size it's given (its header, one ACE's header and mask, the SID),
    // aligned for an ACL; the SID is the token's, and the name is NUL-terminated.
    unsafe {
        let size = (8 + 8 + GetLengthSid(sid) as usize).next_multiple_of(4);
        let mut buffer = vec![0u32; size / 4];
        let acl: Acl = buffer.as_mut_ptr().cast();
        if InitializeAcl(acl, size as u32, ACL_REVISION) == 0 || AddAccessAllowedAceEx(acl, ACL_REVISION, 0, FULL_CONTROL, sid) == 0 {
            return Err("the owner-only rule couldn't be made".into());
        }
        let information = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        let set = SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, information, null_mut(), null_mut(), acl, null_mut());
        if set != 0 {
            return Err(format!("Windows didn't take the owner-only rule (error {set})"));
        }
        if !is_kumis_shape(path) {
            let set =
                SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, sid, null_mut(), null_mut(), null_mut());
            if set != 0 {
                return Err(format!("Windows didn't make you its owner (error {set})"));
            }
        }
    }
    is_kumis_shape(path).then_some(()).ok_or_else(|| "the owner-only rule didn't take".into())
}

/// Shapes for the tests, written raw: each ACE an allow or a deny, with its flags, its mask and whose it is.
#[cfg(test)]
pub(super) mod shapes {
    use super::*;

    pub enum Whose {
        Me,
        Sid(&'static str),
    }
    pub struct Ace {
        pub allow: bool,
        pub flags: u32,
        pub mask: u32,
        pub whose: Whose,
    }
    pub const FULL: u32 = FULL_CONTROL;
    pub const INHERITED: u32 = 0x10;
    pub const INHERITABLE: u32 = 0x3;
    pub const INHERIT_ONLY: u32 = 0x8;

    fn sid_of(text: &str) -> Sid {
        let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut sid = null_mut();
        // SAFETY: the text is NUL-terminated; the SID is Windows' allocation, freed by the caller with LocalFree.
        assert!(unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) } != 0, "{text:?}");
        sid
    }

    /// Makes this account the file's owner, as Kumi's own files have it (an elevated account's new files are
    /// Administrators').
    pub fn own_mine(path: &Path) {
        let account = Account::current().unwrap();
        let name = wide(path);
        // SAFETY: the name is NUL-terminated and the SID the token's.
        let set = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                account.sid(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        assert_eq!(set, 0, "{}", path.display());
    }

    /// Makes `owner` the file's owner; false when this account may not hand it over.
    pub fn own(path: &Path, owner: &str) -> bool {
        let name = wide(path);
        let sid = sid_of(owner);
        // SAFETY: the name is NUL-terminated and the SID valid until it's freed, after the call.
        let set = unsafe {
            SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, sid, null_mut(), null_mut(), null_mut())
        };
        unsafe { LocalFree(sid) };
        set == 0
    }

    /// Writes this DACL on the file as it is, ACE flags and all: SetFileSecurityW takes the descriptor whole, where
    /// SetNamedSecurityInfoW (`write`) tidies a file's ACEs first (it drops inheritance flags and inherit-only ACEs).
    pub fn write_raw(path: &Path, aces: &[Ace]) {
        let account = Account::current().unwrap();
        let name = wide(path);
        unsafe {
            let mut buffer = vec![0u32; 1024];
            let acl: Acl = buffer.as_mut_ptr().cast();
            assert!(InitializeAcl(acl, 4096, ACL_REVISION) != 0);
            for ace in aces {
                let sid = match ace.whose {
                    Whose::Me => account.sid(),
                    Whose::Sid(_) => panic!("raw shapes are this account's"),
                };
                let added = if ace.allow {
                    AddAccessAllowedAceEx(acl, ACL_REVISION, ace.flags, ace.mask, sid)
                } else {
                    AddAccessDeniedAceEx(acl, ACL_REVISION, ace.flags, ace.mask, sid)
                };
                assert!(added != 0);
            }
            // SECURITY_DESCRIPTOR (absolute) is at most 40 bytes on 64-bit Windows.
            let mut descriptor = vec![0u64; 8];
            let descriptor: *mut c_void = descriptor.as_mut_ptr().cast();
            assert!(InitializeSecurityDescriptor(descriptor, 1) != 0);
            assert!(SetSecurityDescriptorDacl(descriptor, 1, acl, 0) != 0);
            assert!(SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED) != 0);
            assert!(SetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION, descriptor) != 0, "{}", path.display());
        }
    }

    /// Writes this DACL on the file: protected or not, and `None` for a null DACL.
    pub fn write(path: &Path, aces: Option<&[Ace]>, protected: bool) {
        let account = Account::current().unwrap();
        let name = wide(path);
        unsafe {
            let mut kept: Vec<Sid> = vec![];
            let mut buffer = vec![0u32; 1024];
            let acl: Acl = match aces {
                None => null_mut(),
                Some(aces) => {
                    let acl: Acl = buffer.as_mut_ptr().cast();
                    assert!(InitializeAcl(acl, 4096, ACL_REVISION) != 0);
                    for ace in aces {
                        let sid = match ace.whose {
                            Whose::Me => account.sid(),
                            Whose::Sid(text) => {
                                let sid = sid_of(text);
                                kept.push(sid);
                                sid
                            }
                        };
                        let added = if ace.allow {
                            AddAccessAllowedAceEx(acl, ACL_REVISION, ace.flags, ace.mask, sid)
                        } else {
                            AddAccessDeniedAceEx(acl, ACL_REVISION, ace.flags, ace.mask, sid)
                        };
                        assert!(added != 0);
                    }
                    acl
                }
            };
            let protection = if protected { PROTECTED_DACL_SECURITY_INFORMATION } else { UNPROTECTED_DACL_SECURITY_INFORMATION };
            let set = SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | protection,
                null_mut(),
                null_mut(),
                acl,
                null_mut(),
            );
            for sid in kept {
                LocalFree(sid);
            }
            assert_eq!(set, 0, "{}", path.display());
        }
    }
}
