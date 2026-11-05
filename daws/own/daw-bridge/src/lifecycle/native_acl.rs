//! Kumi's owner-only folders and files for a DAW's bridge on Windows (Kumi's folder for the DAW, its discovery, logs and
//! generations folders, the install receipt and the journal), read and written with Windows' own calls: a moment's work,
//! where each PowerShell run takes 150 ms on a quiet machine and seconds on the first one after a boot. The lifecycle
//! and doctor fall back to owner.rs for other ACL shapes. Guarded listening also reads this helper in REAPER and
//! Kumi's integration, but fails closed on other shapes: it never starts PowerShell on a DAW or async thread.
//! Its twins are kumi-ai's `auth/windows_acl.rs` and Live's bridge's `lifecycle/delivery/native_acl.rs` (each uses
//! nothing of the others'): keep the three in step. This one also knows a folder's shape.
//!
//! Only Kumi's own shapes are judged here. A file's: this account owns it, and its protected DACL holds one explicit
//! allow ACE, for this account, with full control and no flags (what owner.rs's `SECURE_FILE` writes). A folder's: the
//! same, its one ACE handed down to what's made inside it (ContainerInherit and ObjectInherit, what `SECURE_DIRECTORY`
//! writes). Each is a shape owner.rs's PowerShell check (`FILE_CHECKS`, `DIRECTORY_CHECKS`) passes too. For other shapes,
//! lifecycle and doctor use that check: .NET canonicalizes the ACEs it counts (it merges and drops some), so only it
//! can say no. Guarded listening instead refuses an unrecognized shape without a PowerShell fallback.
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
    fn CreateDirectoryW(path: *const u16, attributes: *const SecurityAttributes) -> i32;
    fn GetFileInformationByHandle(handle: Handle, information: *mut FileInformation) -> i32;
    fn GetFinalPathNameByHandleW(handle: Handle, path: *mut u16, size: u32, flags: u32) -> u32;
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
    fn GetSecurityInfo(
        handle: Handle,
        kind: u32,
        information: u32,
        owner: *mut Sid,
        group: *mut Sid,
        dacl: *mut Acl,
        sacl: *mut Acl,
        descriptor: *mut *mut c_void,
    ) -> u32;
    fn SetSecurityInfo(handle: Handle, kind: u32, information: u32, owner: Sid, group: Sid, dacl: Acl, sacl: Acl) -> u32;
    fn SetSecurityDescriptorOwner(descriptor: *mut c_void, owner: Sid, defaulted: i32) -> i32;
    fn GetSecurityDescriptorControl(descriptor: *mut c_void, control: *mut u16, revision: *mut u32) -> i32;
    fn GetAclInformation(acl: Acl, information: *mut c_void, length: u32, class: u32) -> i32;
    fn GetAce(acl: Acl, index: u32, ace: *mut *mut c_void) -> i32;
    fn EqualSid(left: Sid, right: Sid) -> i32;
    fn GetLengthSid(sid: Sid) -> u32;
    fn InitializeAcl(acl: Acl, length: u32, revision: u32) -> i32;
    fn AddAccessAllowedAceEx(acl: Acl, revision: u32, flags: u32, mask: u32, sid: Sid) -> i32;
    fn AddAccessDeniedAceEx(acl: Acl, revision: u32, flags: u32, mask: u32, sid: Sid) -> i32;
    fn ConvertStringSidToSidW(text: *const u16, sid: *mut Sid) -> i32;
    fn ConvertSecurityDescriptorToStringSecurityDescriptorW(
        descriptor: *mut c_void,
        revision: u32,
        information: u32,
        text: *mut *mut u16,
        length: *mut u32,
    ) -> i32;
    fn InitializeSecurityDescriptor(descriptor: *mut c_void, revision: u32) -> i32;
    fn SetSecurityDescriptorDacl(descriptor: *mut c_void, present: i32, dacl: Acl, defaulted: i32) -> i32;
    fn SetSecurityDescriptorControl(descriptor: *mut c_void, mask: u16, bits: u16) -> i32;
    fn SetFileSecurityW(name: *const u16, information: u32, descriptor: *mut c_void) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSetSecurityObject(handle: Handle, information: u32, descriptor: *mut c_void) -> i32;
}

const DRIVE_REMOTE: u32 = 4;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const SE_FILE_OBJECT: u32 = 1;
const OWNER_SECURITY_INFORMATION: u32 = 0x1;
const DACL_SECURITY_INFORMATION: u32 = 0x4;
const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
const UNPROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x2000_0000;
const SE_DACL_PROTECTED: u16 = 0x1000;
const ACL_SIZE_INFORMATION: u32 = 2;
const ACL_REVISION: u32 = 2;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const TOKEN_QUERY: u32 = 0x8;
const TOKEN_USER: u32 = 1;
/// FileSystemRights.FullControl, what `SECURE_FILE` and `SECURE_DIRECTORY` grant and `FILE_CHECKS` asks for.
const FULL_CONTROL: u32 = 0x001F_01FF;
/// A file's one ACE has no flags; a folder's is handed down to the folders and files made inside it
/// (CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE), as `SECURE_DIRECTORY` writes it.
const FILE_FLAGS: u8 = 0;
const FOLDER_FLAGS: u8 = 0x3;

#[cfg(feature = "test-support")]
type RenderProbe = Box<dyn FnMut(&std::fs::File, bool)>;

thread_local! {
    /// Tests only (`shapes::refuse_on_this_thread`): this thread's writes fail, as a call can, and owner.rs's PowerShell
    /// steps take them.
    static REFUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    #[cfg(feature = "test-support")]
    /// Scoped behavioral probes bracket the real render handle's check/protect interval.
    static RENDER_PROBE: std::cell::RefCell<Option<RenderProbe>> = const { std::cell::RefCell::new(None) };
}

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

/// Whether the file or folder is on this computer's own disk by a plain path: from a drive letter's root, the drive not
/// a share's, and no link or junction on the way to it, its own included. Any other path is left to the PowerShell check.
fn plain_local(path: &Path) -> bool {
    use std::{
        os::windows::fs::MetadataExt,
        path::{Component, Prefix},
    };
    // A drive-relative path ("C:bridge.secret") is the drive's working folder's, which the walk below can't see.
    if !path.has_root() {
        return false;
    }
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

/// Whether the file or folder has Kumi's exact owner-only shape. False also when Windows
/// won't say or the path isn't plain and local. Guarded listening fails closed; lifecycle
/// and doctor may ask the PowerShell check about other shapes.
pub fn is_kumis_shape(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => is_shaped(path, if meta.is_dir() { FOLDER_FLAGS } else { FILE_FLAGS }),
        Err(_) => false,
    }
}

fn is_shaped(path: &Path, flags: u8) -> bool {
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
        let shaped = shaped(account.sid(), owner, dacl, descriptor, flags);
        LocalFree(descriptor);
        shaped
    }
}

/// Kumi's shape, as `FILE_CHECKS` reads a file: owned by `account`; the DACL protected (AreAccessRulesProtected); one ACE
/// (Access.Count), for `account`, an allow ACE, with full control exactly, and its flags `flags` exactly (none for a
/// file, so not inherited; handed down for a folder): what SECURE_FILE and SECURE_DIRECTORY write, and nothing more.
/// (`DIRECTORY_CHECKS` asks only for the owner and the protection, which this shape has.)
///
/// # Safety
/// `owner`, `dacl` and `descriptor` come from one GetNamedSecurityInfoW that succeeded, and are still allocated.
unsafe fn shaped(account: Sid, owner: Sid, dacl: Acl, descriptor: *mut c_void, flags: u8) -> bool {
    unsafe {
        let (mut control, mut revision) = (0u16, 0u32);
        GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) != 0
            && control & SE_DACL_PROTECTED != 0
            && owner_rule(account, owner, dacl, flags)
    }
}

/// The single full-control owner ACE, shared by protected and inherited-file checks.
/// # Safety
/// The owner and DACL point into a live security descriptor supplied by Windows.
unsafe fn owner_rule(account: Sid, owner: Sid, dacl: Acl, flags: u8) -> bool {
    unsafe {
        if owner.is_null() || EqualSid(owner, account) == 0 {
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
        if header.ace_type != ACCESS_ALLOWED_ACE_TYPE || header.ace_flags != flags {
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
    make(path, FILE_FLAGS)
}

/// Gives the folder Kumi's shape, as `SECURE_DIRECTORY` does: the same, its one ACE handed down to what's made inside
/// it. Then it's checked. Err when a step fails, and PowerShell's own step runs instead.
pub fn make_kumis_folder_shape(path: &Path) -> Result<(), String> {
    if !std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
        return Err("it isn't a folder".into());
    }
    make(path, FOLDER_FLAGS)
}

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

/// Creates exactly one new directory with the protected owner-only ACL from its first
/// visible moment. An existing directory is never repaired; no PowerShell fallback.
pub fn create_kumis_folder(path: &Path) -> std::io::Result<()> {
    if path.parent().is_none_or(|parent| !plain_local(parent)) || path.as_os_str().encode_wide().any(|c| c == 0) {
        return Err(std::io::Error::other("not a plain local directory path"));
    }
    let account = Account::current().ok_or_else(|| std::io::Error::other("the current account could not be read"))?;
    let mut buffer = owner_acl(&account, FOLDER_FLAGS).map_err(std::io::Error::other)?;
    let name = wide(path);
    // SECURITY_DESCRIPTOR is at most 40 bytes on 64-bit Windows, aligned for pointers.
    let mut descriptor = [0u64; 5];
    let descriptor = descriptor.as_mut_ptr().cast();
    // SAFETY: descriptor, ACL, SID and NUL-terminated name live through CreateDirectoryW.
    unsafe {
        if InitializeSecurityDescriptor(descriptor, 1) == 0
            || SetSecurityDescriptorOwner(descriptor, account.sid(), 0) == 0
            || SetSecurityDescriptorDacl(descriptor, 1, buffer.as_mut_ptr().cast(), 0) == 0
            || SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let attributes = SecurityAttributes { length: std::mem::size_of::<SecurityAttributes>() as u32, descriptor, inherit: 0 };
        if CreateDirectoryW(name.as_ptr(), &attributes) == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[repr(C)]
#[derive(Default)]
struct FileInformation {
    attributes: u32,
    creation: [u32; 2],
    access: [u32; 2],
    write: [u32; 2],
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

fn file_information(file: &std::fs::File) -> Result<FileInformation, String> {
    use std::os::windows::io::AsRawHandle;
    let mut info = FileInformation::default();
    // SAFETY: the output has BY_HANDLE_FILE_INFORMATION's layout and the handle is live.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err("the created output's identity could not be read".into());
    }
    Ok(info)
}

/// Protects an exact newly produced render file, never a pathname that can be replaced.
/// The caller owns the creation, holds `original` from before its rename, and pins
/// `file` with FILE_READ_DATA, READ_CONTROL and WRITE_DAC, without write/delete sharing.
/// Existing cache entries must not use this.
pub fn protect_created_file(original: &std::fs::File, file: &std::fs::File) -> Result<(), String> {
    protect_render(original, file, Protection::CreatedFile)
}

/// An owning component may protect its dedicated listening directory at startup; an
/// owning plugin runtime may protect an exact directory in its own --state-dir tree.
/// Both handles must pin the same current-user-owned, non-reparse directory, with
/// FILE_LIST_DIRECTORY access and no write/delete sharing; `directory` also needs WRITE_DAC.
/// Only this entry's DACL changes: no ownership takeover or propagation to old children.
/// This is never authority to repair an adopted source, an ancestor or a neighbor.
pub fn protect_owned_directory(original: &std::fs::File, directory: &std::fs::File) -> Result<(), String> {
    protect_pinned(original, directory, Protection::OwnedDirectory)
}

/// The normalized path of an opened entry, used to bind a pinned cache to its pinned
/// parent rather than trusting a second lookup of either pathname.
pub fn file_path(file: &std::fs::File) -> Result<std::path::PathBuf, String> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    // SAFETY: query the live handle's buffer size, then write at most that size.
    let size = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), null_mut(), 0, 0) };
    if size == 0 || size > 32768 {
        return Err("the pinned staging path could not be established".into());
    }
    let mut path = vec![0u16; size as usize];
    let written = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), path.as_mut_ptr(), size, 0) };
    if written == 0 || written >= size {
        return Err("the pinned staging path changed while being read".into());
    }
    Ok(std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..written as usize])))
}

/// Checks the exact private shape of a pinned root or cache without resolving its path.
pub fn is_kumis_shape_handle(file: &std::fs::File, directory: bool) -> bool {
    use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
    let Ok(meta) = file.metadata() else { return false };
    if (if directory { !meta.is_dir() } else { !meta.is_file() }) || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return false;
    }
    let Some(account) = Account::current() else { return false };
    // SAFETY: Windows' descriptor and internal pointers remain live until LocalFree.
    unsafe {
        let (mut owner, mut dacl, mut descriptor) = (null_mut(), null_mut(), null_mut());
        if GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        ) != 0
        {
            return false;
        }
        let private = shaped(account.sid(), owner, dacl, descriptor, if directory { FOLDER_FLAGS } else { FILE_FLAGS });
        LocalFree(descriptor);
        private
    }
}

/// Protects an owned completed Renoise render only if its starting ACL is exactly the
/// single owner FullControl ACE inherited from its secured cache, or already protected.
/// The caller proves job ownership/containment and opens with FILE_READ_DATA,
/// READ_CONTROL and WRITE_DAC, without write/delete sharing, before any checks.
pub fn protect_inherited_render(file: &std::fs::File) -> Result<(), String> {
    protect_render(file, file, Protection::InheritedRender)
}

#[derive(Clone, Copy)]
enum Protection {
    CreatedFile,
    OwnedDirectory,
    InheritedRender,
}

fn protect_render(original: &std::fs::File, file: &std::fs::File, protection: Protection) -> Result<(), String> {
    #[cfg(feature = "test-support")]
    let mut probe = RENDER_PROBE.with(|slot| slot.borrow_mut().take());
    #[cfg(feature = "test-support")]
    if let Some(probe) = probe.as_mut() {
        probe(file, false);
    }
    let result = protect_pinned(original, file, protection);
    #[cfg(feature = "test-support")]
    if result.is_ok() {
        if let Some(probe) = probe.as_mut() {
            probe(file, true);
        }
    }
    result
}

fn protect_pinned(original: &std::fs::File, file: &std::fs::File, protection: Protection) -> Result<(), String> {
    use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
    let directory = matches!(protection, Protection::OwnedDirectory);
    let inherited = matches!(protection, Protection::InheritedRender);
    let meta = file.metadata().map_err(|_| "the pinned staging entry could not be inspected")?;
    if (if directory { !meta.is_dir() } else { !meta.is_file() }) || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("the pinned staging entry has the wrong type or is a reparse point".into());
    }
    let now = file_information(file)?;
    let same = if original.as_raw_handle() == file.as_raw_handle() {
        true
    } else {
        let before = file_information(original)?;
        before.volume == now.volume && before.index_high == now.index_high && before.index_low == now.index_low
    };
    if (!directory && now.links != 1) || !same {
        return Err("the pinned staging entry was replaced or hard-linked".into());
    }
    let account = Account::current().ok_or("the current account couldn't be read")?;
    let flags = if directory { FOLDER_FLAGS } else { FILE_FLAGS };
    let handle = file.as_raw_handle();
    // SAFETY: the open handle, ACL and SID remain alive; Windows owns the descriptor
    // returned by GetSecurityInfo and it is released after the shared shape check.
    unsafe {
        let (mut owner, mut dacl, mut descriptor) = (null_mut(), null_mut(), null_mut());
        let information = OWNER_SECURITY_INFORMATION | if inherited { DACL_SECURITY_INFORMATION } else { 0 };
        if GetSecurityInfo(handle, SE_FILE_OBJECT, information, &mut owner, null_mut(), &mut dacl, null_mut(), &mut descriptor) != 0 {
            return Err("the pinned staging entry's owner could not be read".into());
        }
        let owned = !owner.is_null() && EqualSid(owner, account.sid()) != 0;
        let private = inherited && shaped(account.sid(), owner, dacl, descriptor, FILE_FLAGS);
        let (mut control, mut revision) = (0u16, 0u32);
        let accepted = !inherited
            || private
            || (GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) != 0
                && control & SE_DACL_PROTECTED == 0
                && owner_rule(account.sid(), owner, dacl, 0x10)); // INHERITED_ACE only
        LocalFree(descriptor);
        if !owned {
            return Err("the pinned staging entry belongs to another account".into());
        }
        if !accepted {
            return Err("the render's starting ACL is not the cache's single inherited owner rule".into());
        }
        if private {
            return Ok(()); // Already read-confirmed; do not rewrite a completed file's ACL.
        }
        let mut buffer = owner_acl(&account, flags)?;
        let changed = if directory {
            // SetSecurityInfo walks existing children; sharing mode is not an ACL
            // propagation boundary. Set only this pinned object's descriptor instead.
            // https://learn.microsoft.com/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntsetsecurityobject
            let mut descriptor = [0u64; 5]; // Absolute SECURITY_DESCRIPTOR, pointer-aligned.
            let descriptor = descriptor.as_mut_ptr().cast();
            InitializeSecurityDescriptor(descriptor, 1) != 0
                && SetSecurityDescriptorDacl(descriptor, 1, buffer.as_mut_ptr().cast(), 0) != 0
                && SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED) != 0
                && NtSetSecurityObject(handle, DACL_SECURITY_INFORMATION, descriptor) >= 0
        } else {
            let information = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
            SetSecurityInfo(handle, SE_FILE_OBJECT, information, null_mut(), null_mut(), buffer.as_mut_ptr().cast(), null_mut()) == 0
        };
        if !changed {
            return Err("the pinned staging entry could not be protected".into());
        }
        let (mut owner, mut dacl, mut descriptor) = (null_mut(), null_mut(), null_mut());
        if GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        ) != 0
        {
            return Err("the pinned staging entry's protection could not be read back".into());
        }
        let private = shaped(account.sid(), owner, dacl, descriptor, flags);
        LocalFree(descriptor);
        private.then_some(()).ok_or_else(|| "the pinned staging entry is not owner-only".into())
    }
}

fn owner_acl(account: &Account, flags: u8) -> Result<Vec<u32>, String> {
    // SAFETY: the account owns the SID; the aligned ACL buffer is sized for its one ACE.
    unsafe {
        let size = (8 + 8 + GetLengthSid(account.sid()) as usize).next_multiple_of(4);
        let mut buffer = vec![0u32; size / 4];
        let acl = buffer.as_mut_ptr().cast();
        if InitializeAcl(acl, size as u32, ACL_REVISION) == 0
            || AddAccessAllowedAceEx(acl, ACL_REVISION, flags as u32, FULL_CONTROL, account.sid()) == 0
        {
            return Err("the owner-only rule couldn't be made".into());
        }
        Ok(buffer)
    }
}

fn make(path: &Path, flags: u8) -> Result<(), String> {
    if REFUSED.with(std::cell::Cell::get) {
        return Err("refused on this thread (a test's)".into());
    }
    // Nothing is written where the check would leave the verdict to PowerShell anyway (a share, a link on the way).
    if !plain_local(path) {
        return Err("it isn't on this computer's own disk by a plain path".into());
    }
    let account = Account::current().ok_or("this account couldn't be read")?;
    let sid = account.sid();
    let name = wide(path);
    // SAFETY: the ACL is built in a buffer of the size it's given (its header, one ACE's header and mask, the SID),
    // aligned for an ACL; the SID is the token's, and the name is NUL-terminated.
    unsafe {
        let mut buffer = owner_acl(&account, flags)?;
        let acl: Acl = buffer.as_mut_ptr().cast();
        let information = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        let set = SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, information, null_mut(), null_mut(), acl, null_mut());
        if set != 0 {
            return Err(format!("Windows didn't take the owner-only rule (error {set})"));
        }
        if !owned_by(&name, sid) {
            let set =
                SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, sid, null_mut(), null_mut(), null_mut());
            if set != 0 {
                return Err(format!("Windows didn't make you its owner (error {set})"));
            }
        }
    }
    is_shaped(path, flags).then_some(()).ok_or_else(|| "the owner-only rule didn't take".into())
}

/// Whether the file at `name` (NUL-terminated) is owned by `account`.
fn owned_by(name: &[u16], account: Sid) -> bool {
    let (mut owner, mut descriptor) = (null_mut(), null_mut());
    // SAFETY: on success the descriptor is Windows' own allocation, which the owner points into, read before it's freed.
    unsafe {
        let read = GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        );
        if read != 0 {
            return false;
        }
        let owned = !owner.is_null() && EqualSid(owner, account) != 0;
        LocalFree(descriptor);
        owned
    }
}

/// Shapes for daw-bridge's tests (tests/it/system.rs), written raw: each ACE an allow or a deny, with its flags, its mask
/// and whose it is. Not for Kumi itself.
#[doc(hidden)]
pub mod shapes {
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

    /// Makes this thread's writes fail, as one of Windows' calls can, so that owner.rs's PowerShell steps take them.
    pub fn refuse_on_this_thread(refused: bool) {
        REFUSED.with(|cell| cell.set(refused));
    }

    #[cfg(feature = "test-support")]
    /// Clears an unused probe on its installing thread, including when a test unwinds.
    pub struct RenderProbeGuard(std::marker::PhantomData<std::rc::Rc<()>>);

    #[cfg(feature = "test-support")]
    impl Drop for RenderProbeGuard {
        fn drop(&mut self) {
            RENDER_PROBE.with(|slot| drop(slot.borrow_mut().take()));
        }
    }

    #[cfg(feature = "test-support")]
    /// Probes the next real render protection on this thread, with the same handle
    /// before its first check (`false`) and after successful ACL read-back (`true`).
    pub fn probe_next_render(probe: impl FnMut(&std::fs::File, bool) + 'static) -> RenderProbeGuard {
        RENDER_PROBE.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(slot.is_none(), "a render protection probe is already installed");
            *slot = Some(Box::new(probe));
        });
        RenderProbeGuard(std::marker::PhantomData)
    }

    /// Stable owner/DACL evidence for behavioral regressions, without PowerShell.
    pub fn descriptor(path: &Path) -> String {
        let name = wide(path);
        let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        // SAFETY: Windows allocates both the descriptor and converted text; both remain
        // live while read and are then freed exactly once.
        unsafe {
            let mut descriptor = null_mut();
            assert_eq!(
                GetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    information,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    &mut descriptor
                ),
                0
            );
            let (mut text, mut length) = (null_mut(), 0u32);
            let converted = ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, 1, information, &mut text, &mut length);
            LocalFree(descriptor);
            assert_ne!(converted, 0);
            assert!(length > 0);
            let result = String::from_utf16_lossy(std::slice::from_raw_parts(text, length as usize - 1));
            LocalFree(text.cast());
            result
        }
    }

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
