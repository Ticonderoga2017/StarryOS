use alloc::{sync::Arc, vec::Vec};
use core::{
    mem::{MaybeUninit, transmute},
    slice::from_raw_parts,
};

use axerrno::{AxError, AxResult};
use linux_raw_sys::net::{SCM_RIGHTS, SOL_SOCKET, cmsghdr};
use starry_vm::{vm_read_slice, vm_write_slice};

use crate::file::{FileLike, get_file_like};

pub enum CMsg {
    Rights { fds: Vec<Arc<dyn FileLike>> },
}
impl CMsg {
    pub fn parse(hdr: &cmsghdr) -> AxResult<Self> {
        if hdr.cmsg_len < size_of::<cmsghdr>() {
            return Err(AxError::InvalidInput);
        }

        let data_len = hdr.cmsg_len - size_of::<cmsghdr>();
        let data_ptr = (hdr as *const cmsghdr as usize + size_of::<cmsghdr>()) as *const u8;
        let mut data = Vec::with_capacity(data_len);
        unsafe {
            data.set_len(data_len);
        }
        vm_read_slice(data_ptr, unsafe {
            transmute::<&mut [u8], &mut [MaybeUninit<u8>]>(&mut data)
        })?;

        Ok(match (hdr.cmsg_level as u32, hdr.cmsg_type as u32) {
            (SOL_SOCKET, SCM_RIGHTS) => {
                if data.len() % size_of::<i32>() != 0 {
                    return Err(AxError::InvalidInput);
                }
                let mut fds = Vec::new();
                for fd in data.chunks_exact(size_of::<i32>()) {
                    let fd = i32::from_ne_bytes(fd.try_into().unwrap());
                    if fd < 0 {
                        return Err(AxError::BadFileDescriptor);
                    }
                    let f = get_file_like(fd)?;
                    fds.push(f);
                }
                Self::Rights { fds }
            }
            _ => {
                return Err(AxError::InvalidInput);
            }
        })
    }
}

pub struct CMsgBuilder<'a> {
    hdr: *mut cmsghdr,
    len: &'a mut usize,
    capacity: usize,
}
impl<'a> CMsgBuilder<'a> {
    pub fn new(msg: *mut cmsghdr, len: &'a mut usize) -> Self {
        let capacity = *len;
        *len = 0;
        Self {
            hdr: msg,
            len,
            capacity,
        }
    }

    pub fn push(
        &mut self,
        level: u32,
        ty: u32,
        body: impl FnOnce(&mut [u8]) -> AxResult<usize>,
    ) -> AxResult<bool> {
        let Some(body_capacity) = (self.capacity - *self.len).checked_sub(size_of::<cmsghdr>())
        else {
            return Ok(false);
        };

        let data_ptr = ((self.hdr as usize) + size_of::<cmsghdr>()) as *mut u8;
        let mut data = Vec::with_capacity(body_capacity);
        unsafe {
            data.set_len(body_capacity);
        }
        let body_len = body(&mut data)?;
        vm_write_slice(data_ptr, &data[..body_len])?;

        let cmsg_len = size_of::<cmsghdr>() + body_len;
        let hdr = cmsghdr {
            cmsg_len,
            cmsg_level: level as _,
            cmsg_type: ty as _,
        };
        unsafe {
            let hdr_bytes = from_raw_parts(&hdr as *const _ as *const u8, size_of::<cmsghdr>());
            vm_write_slice(self.hdr as *mut u8, hdr_bytes)?;
        }

        self.hdr = (self.hdr as usize + cmsg_len) as *mut cmsghdr;
        *self.len += cmsg_len;
        Ok(true)
    }
}
