//! [`Request::Open`](fsproto::Request::Open): a path beneath a `Dir`, as cap-std
//! resolves one. Each name is looked up in turn; `.` stays, `..` goes back up the
//! names walked so far and never above the `Dir` itself, and an absolute or
//! empty name is refused. So a `Dir` reaches only what lies beneath it.

use super::{BASE, Call, DIR_RIGHTS, FILE_RIGHTS, Mint, Server, error, stat};
use alloc::vec;
use fsproto::{Error, How, Reply};
use kabi::handle::Rights;
use kaifs::items::valid_name;
use kaifs::{BlockDev, FileType};

impl<D: BlockDev> Server<D> {
    pub(crate) fn open(
        &mut self,
        call: &Call,
        dir: u64,
        path: &str,
        rights: Rights,
        how: How,
    ) -> Result<(Reply, Option<Mint>), Error> {
        if !call.rights.contains(rights) {
            return Err(Error::Permission);
        }
        if how != How::Existing && !call.rights.contains(Rights::CREATE) {
            return Err(Error::Permission);
        }
        let (parent, last) = self.walk(dir, path)?;
        let fs = self.fs();
        let object = match (how, last) {
            (How::Existing, None) => parent,
            (How::Existing, Some(name)) => {
                fs.lookup(parent, name)
                    .map_err(error)?
                    .ok_or(Error::NotFound)?
                    .ino
            }
            (How::CreateFile, Some(name)) => fs.create(parent, name).map_err(error)?,
            (How::Mkdir, Some(name)) => fs.mkdir(parent, name).map_err(error)?,
            (_, None) => return Err(Error::Exists),
        };
        let inode = fs.stat(object).map_err(error)?;
        let file = inode.kind == FileType::File;
        let volume = fs.volume() as u8;
        // The kind's rights the caller asked for, and always the base ones: REPLY_H
        // moves a handle only with TRANSFER, and the client may pass it on or
        // narrow it as any handle.
        let rights = (rights & if file { FILE_RIGHTS } else { DIR_RIGHTS }) | BASE;
        let badge = self.badge(volume, file, object)?;
        let mint = Mint {
            file,
            rights,
            badge,
        };
        Ok((Reply::Opened(stat(object, &inode), badge), Some(mint)))
    }

    /// The folder holding `path`'s last name, and that name: `None` when the
    /// path names a folder walked to (`""`, `.`, `a/..`).
    fn walk<'p>(&mut self, dir: u64, path: &'p str) -> Result<(u64, Option<&'p str>), Error> {
        if path.starts_with('/') {
            return Err(Error::Invalid);
        }
        let parts: alloc::vec::Vec<&str> = if path.is_empty() {
            vec![]
        } else {
            path.split('/').collect()
        };
        let mut walked = vec![dir];
        for (at, part) in parts.iter().enumerate() {
            let here = *walked.last().expect("never empty");
            match *part {
                "." => {}
                ".." => {
                    if walked.len() == 1 {
                        return Err(Error::Invalid);
                    }
                    walked.pop();
                }
                name if !valid_name(name) => return Err(Error::Invalid),
                name if at + 1 == parts.len() => return Ok((here, Some(name))),
                name => {
                    let entry = self.fs().lookup(here, name).map_err(error)?;
                    let entry = entry.ok_or(Error::NotFound)?;
                    if entry.kind != FileType::Dir {
                        return Err(Error::NotDir);
                    }
                    walked.push(entry.ino);
                }
            }
        }
        Ok((*walked.last().expect("never empty"), None))
    }
}
