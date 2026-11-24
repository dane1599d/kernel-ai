//! Bulk reads and writes: [`fsproto::Request::ReadShm`] and [`fsproto::Request::WriteShm`] carry
//! up to [`MAX_SHM_DATA`] of file data in an `Shm` the client moves with the
//! request, where [`fsproto::Request::Read`] and [`fsproto::Request::Write`] carry 7 KiB in the
//! message. One round trip then moves 1 MiB instead of 7 KiB: the IPC round
//! trip, not kaifs, is most of what a 7 KiB message costs.
//!
//! The rights, the action rules and the pending run are [`fsproto::Request::Write`]'s,
//! since both go through [`Server::write`] and [`Server::read`]; only where the
//! bytes are differs. `fs` maps the `Shm` for the one request and unmaps it.

use super::{Call, MAX_SHM_DATA, Server, Target};
use fsproto::{Error, Reply};
use kabi::handle::Rights;
use kaifs::BlockDev;

impl<D: BlockDev> Server<D> {
    /// [`fsproto::Request::ReadShm`]: up to `len` bytes from `off` into `shm`'s start.
    pub(crate) fn read_shm(
        &mut self,
        call: &Call,
        target: &Target,
        off: u64,
        len: u32,
        shm: Option<&mut [u8]>,
    ) -> Result<Reply, Error> {
        let shm = bulk_target(call, target, Rights::READ, shm)?;
        let len = (len as usize).min(MAX_SHM_DATA).min(shm.len());
        let data = self.read(target.object, off, len, target.inode.size)?;
        shm[..data.len()].copy_from_slice(&data);
        Ok(Reply::Copied {
            len: data.len() as u64,
        })
    }

    /// [`fsproto::Request::WriteShm`]: `shm`'s first `len` bytes at `off`.
    pub(crate) fn write_shm(
        &mut self,
        call: &Call,
        target: &Target,
        off: u64,
        len: u32,
        shm: Option<&mut [u8]>,
    ) -> Result<Reply, Error> {
        let shm = bulk_target(call, target, Rights::WRITE, shm)?;
        let len = len as usize;
        if len > MAX_SHM_DATA || len > shm.len() {
            return Err(Error::Invalid);
        }
        self.write(target.object, off, &shm[..len])?;
        Ok(Reply::Done)
    }
}

/// The `Shm`, once the handle is a `File` with `rights`: Invalid with none.
fn bulk_target<'a>(
    call: &Call,
    target: &Target,
    rights: Rights,
    shm: Option<&'a mut [u8]>,
) -> Result<&'a mut [u8], Error> {
    if !target.file {
        return Err(Error::IsDir);
    }
    if !call.rights.contains(rights) {
        return Err(Error::Permission);
    }
    shm.ok_or(Error::Invalid)
}
