//! Check the opened Windows object, avoiding path races and a PowerShell process for every poll.
use std::{
    ffi::c_void,
    fs::File,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::Path,
};
type Handle = *mut c_void;
#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(token: Handle, class: u32, buffer: *mut c_void, size: u32, needed: *mut u32) -> i32;
    fn GetSecurityInfo(
        handle: Handle,
        object: u32,
        requested: u32,
        owner: *mut Handle,
        group: *mut Handle,
        dacl: *mut Handle,
        sacl: *mut Handle,
        descriptor: *mut Handle,
    ) -> u32;
    fn GetSecurityDescriptorControl(descriptor: Handle, control: *mut u16, revision: *mut u32) -> i32;
    fn GetAclInformation(acl: Handle, buffer: *mut c_void, size: u32, class: u32) -> i32;
    fn GetAce(acl: Handle, index: u32, ace: *mut Handle) -> i32;
    fn EqualSid(left: Handle, right: Handle) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn LocalFree(memory: Handle) -> Handle;
}
struct Token(Handle);
impl Drop for Token {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Descriptor(Handle);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
#[repr(C)]
struct AclSize {
    count: u32,
    used: u32,
    free: u32,
}

pub fn private_directory(path: &Path) -> bool {
    let file = std::fs::OpenOptions::new().read(true).share_mode(7).custom_flags(0x0020_0000 | 0x0200_0000).open(path);
    file.is_ok_and(|file| file.metadata().is_ok_and(|meta| meta.is_dir() && meta.file_attributes() & 0x400 == 0) && private_file(&file))
}

/// A protected DACL with exactly one non-inherited full-control allow ACE for this user's SID.
pub fn private_file(file: &File) -> bool {
    // SAFETY: every Win32 output has valid storage, token/descriptor allocations outlive their SID/ACL pointers,
    // and both owned native resources are released by their guards on all exits. ACE size is checked before reading.
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 {
            return false;
        }
        let token = Token(token);
        let mut needed = 0;
        GetTokenInformation(token.0, 1, std::ptr::null_mut(), 0, &mut needed);
        if needed < std::mem::size_of::<usize>() as u32 || needed > 65536 {
            return false;
        }
        let mut user = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(token.0, 1, user.as_mut_ptr().cast(), needed, &mut needed) == 0 {
            return false;
        }
        let sid = *(user.as_ptr().cast::<Handle>());
        let mut owner = std::ptr::null_mut();
        let mut acl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        if GetSecurityInfo(
            file.as_raw_handle().cast(),
            1,
            5,
            &mut owner,
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor,
        ) != 0
        {
            return false;
        }
        let descriptor = Descriptor(descriptor);
        if owner.is_null() || acl.is_null() || EqualSid(owner, sid) == 0 {
            return false;
        }
        let mut control = 0;
        let mut revision = 0;
        if GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) == 0 || control & 0x1000 == 0 {
            return false;
        }
        let mut size = AclSize { count: 0, used: 0, free: 0 };
        if GetAclInformation(acl, (&mut size as *mut AclSize).cast(), std::mem::size_of::<AclSize>() as u32, 2) == 0 || size.count != 1 {
            return false;
        }
        let mut ace = std::ptr::null_mut();
        if GetAce(acl, 0, &mut ace) == 0 || ace.is_null() {
            return false;
        }
        let bytes = ace.cast::<u8>();
        let ace_size = u16::from_le_bytes([*bytes.add(2), *bytes.add(3)]) as usize;
        if *bytes != 0 || *bytes.add(1) & 0x10 != 0 || ace_size < 16 {
            return false;
        }
        let subauthorities = *bytes.add(9) as usize;
        if *bytes.add(8) != 1 || subauthorities > 15 || 16 + 4 * subauthorities > ace_size {
            return false;
        }
        let mask = std::ptr::read_unaligned(bytes.add(4).cast::<u32>());
        mask & 0x001f01ff == 0x001f01ff && EqualSid(bytes.add(8).cast(), sid) != 0
    }
}
