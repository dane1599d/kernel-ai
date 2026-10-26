//! The WASI preview-1 calls xcc's libc declares (`libsrc/_wasm/wasi.h` at xcc
//! 631e318), for the compiler's Store in `cc` (ADR 0012 D2). Twenty calls, over an
//! in-memory [`Tree`]: `/kai` read-only and `/w` scratch, preopened as fds 3 and 4.
//!
//! - The clock always reads 0, so a compile cannot tell the time.
//! - `random_get` is refused with `NOSYS`.
//! - `proc_exit` ends the compile: [`Wasi::call`] returns [`Stop::Exit`], which the
//!   glue turns into a trap.
//! - Output to fds 1 and 2 is kept up to [`Limits::output_bytes`] each; the rest is
//!   dropped (and reported as written, so the guest does not retry), and
//!   [`Wasi::output_truncated`] says so.
//!
//! Every guest pointer and length is checked against memory before use (`mem.rs`),
//! every guest path is joined without `..` (`tree.rs`), and every count is bounded.
//! No Wasm engine is named here: a call takes the guest's memory as a byte slice
//! and its arguments as `u64`s, so the engine glue stays in `cc`.

#![no_std]

extern crate alloc;

mod errno;
mod mem;
mod tree;

#[cfg(test)]
mod tests;

pub use errno::Errno;
pub use tree::{KAI, Kind, SCRATCH, Tree, TreeLimits};

use alloc::string::String;
use alloc::vec::Vec;
use errno::Res;
use mem::Guest;

/// The import module every call is in.
pub const MODULE: &str = "wasi_snapshot_preview1";
/// Longest guest path, in bytes.
pub const MAX_PATH: usize = 1024;
/// Most iovec entries one read or write may pass.
pub const MAX_IOVS: u32 = 1024;
/// Most bytes of arguments, with their NULs.
pub const MAX_ARGS_BYTES: usize = 4096;

const FD_STDOUT: u32 = 1;
const FD_STDERR: u32 = 2;
/// `/kai` and `/w`, in this order, from fd 3.
const PREOPENS: [&str; 2] = [KAI, SCRATCH];

const RIGHT_FD_READ: u64 = 1 << 1;
const RIGHT_FD_WRITE: u64 = 1 << 6;
const OFLAGS_CREAT: u32 = 1 << 0;
const OFLAGS_DIRECTORY: u32 = 1 << 1;
const OFLAGS_EXCL: u32 = 1 << 2;
const OFLAGS_TRUNC: u32 = 1 << 3;
const FDFLAGS_APPEND: u32 = 1 << 0;
const FILETYPE_CHARACTER_DEVICE: u8 = 2;
const FILETYPE_DIRECTORY: u8 = 3;
const FILETYPE_REGULAR_FILE: u8 = 4;
/// The highest clock id preview 1 defines (thread CPU time).
const MAX_CLOCK_ID: u32 = 3;

/// A Wasm value type of a call's parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    I32,
    I64,
}

/// One import: its field name, parameters, and whether it returns an `i32` errno.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Import {
    pub name: &'static str,
    pub params: &'static [Ty],
    pub result: bool,
}

use Ty::{I32, I64};

const fn import(name: &'static str, params: &'static [Ty]) -> Import {
    Import {
        name,
        params,
        result: true,
    }
}

/// The twenty calls, with the signatures preview 1 gives them.
pub const IMPORTS: &[Import] = &[
    import("args_sizes_get", &[I32, I32]),
    import("args_get", &[I32, I32]),
    import("environ_sizes_get", &[I32, I32]),
    import("environ_get", &[I32, I32]),
    Import {
        name: "proc_exit",
        params: &[I32],
        result: false,
    },
    import("fd_prestat_get", &[I32, I32]),
    import("fd_prestat_dir_name", &[I32, I32, I32]),
    import("path_open", &[I32, I32, I32, I32, I32, I64, I64, I32, I32]),
    import("path_unlink_file", &[I32, I32, I32]),
    import("path_rename", &[I32, I32, I32, I32, I32, I32]),
    import("path_create_directory", &[I32, I32, I32]),
    import("path_remove_directory", &[I32, I32, I32]),
    import("path_filestat_get", &[I32, I32, I32, I32, I32]),
    import("fd_read", &[I32, I32, I32, I32]),
    import("fd_write", &[I32, I32, I32, I32]),
    import("fd_close", &[I32]),
    import("fd_seek", &[I32, I64, I32, I32]),
    import("fd_filestat_get", &[I32, I32]),
    import("clock_time_get", &[I32, I64, I32]),
    import("random_get", &[I32, I32]),
];

/// The import named `name`, if it is one of the twenty.
pub fn lookup(name: &str) -> Option<&'static Import> {
    IMPORTS.iter().find(|import| import.name == name)
}

/// Why a call did not return an errno.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// `proc_exit`: the compile is over, with this status.
    Exit(u32),
    /// Not one of the twenty, or the wrong number of arguments.
    BadCall,
}

/// Limits on one compile's WASI state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Most open fds, stdio and preopens included.
    pub fds: usize,
    /// Bytes kept from each of stdout and stderr.
    pub output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fds: 32,
            output_bytes: 64 << 10,
        }
    }
}

#[derive(Clone, Debug)]
enum Fd {
    Stdin,
    Out(u32),
    Dir(String),
    File {
        path: String,
        pos: u64,
        read: bool,
        write: bool,
        append: bool,
    },
}

/// One compile's WASI state: the tree, open fds, arguments and captured output.
pub struct Wasi {
    tree: Tree,
    fds: Vec<Option<Fd>>,
    args: Vec<u8>,
    argc: u32,
    limits: Limits,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
    exit: Option<u32>,
}

impl Wasi {
    /// A fresh state over `tree` with `args` (argv\[0\] included). Arguments longer
    /// than [`MAX_ARGS_BYTES`] in total, or containing a NUL, are refused.
    pub fn new(tree: Tree, args: &[&str], limits: Limits) -> Result<Self, Errno> {
        let mut packed = Vec::new();
        for arg in args {
            if arg.contains('\0') {
                return Err(Errno::INVAL);
            }
            packed.extend_from_slice(arg.as_bytes());
            packed.push(0);
        }
        if packed.len() > MAX_ARGS_BYTES || limits.fds < 3 + PREOPENS.len() {
            return Err(Errno::INVAL);
        }
        let mut fds = alloc::vec![Some(Fd::Stdin), Some(Fd::Out(1)), Some(Fd::Out(2))];
        fds.extend(PREOPENS.iter().map(|p| Some(Fd::Dir(String::from(*p)))));
        Ok(Self {
            tree,
            fds,
            args: packed,
            argc: args.len() as u32,
            limits,
            stdout: Vec::new(),
            stderr: Vec::new(),
            truncated: false,
            exit: None,
        })
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn tree_mut(&mut self) -> &mut Tree {
        &mut self.tree
    }

    /// The tree, for the next compile.
    pub fn into_tree(self) -> Tree {
        self.tree
    }

    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    /// Whether output past [`Limits::output_bytes`] was dropped.
    pub fn output_truncated(&self) -> bool {
        self.truncated
    }

    /// The status `proc_exit` was called with, if it was.
    pub fn exit_code(&self) -> Option<u32> {
        self.exit
    }

    /// Runs import `name` with `args` (each `i32` in the low 32 bits) over the
    /// guest's `memory`; returns its errno, or why it stopped.
    pub fn call(&mut self, name: &str, memory: &mut [u8], args: &[u64]) -> Result<u16, Stop> {
        let import = lookup(name).ok_or(Stop::BadCall)?;
        if args.len() != import.params.len() {
            return Err(Stop::BadCall);
        }
        let a = |i: usize| args[i] as u32;
        let mut mem = Guest(memory);
        let mem = &mut mem;
        let result = match name {
            "args_sizes_get" => self.args_sizes_get(mem, a(0), a(1)),
            "args_get" => self.args_get(mem, a(0), a(1)),
            "environ_sizes_get" => environ_sizes_get(mem, a(0), a(1)),
            "environ_get" => Ok(()),
            "proc_exit" => {
                self.exit = Some(a(0));
                return Err(Stop::Exit(a(0)));
            }
            "fd_prestat_get" => self.fd_prestat_get(mem, a(0), a(1)),
            "fd_prestat_dir_name" => self.fd_prestat_dir_name(mem, a(0), a(1), a(2)),
            "path_open" => self.path_open(mem, args),
            "path_unlink_file" => self.path_op(mem, a(0), a(1), a(2), Tree::unlink),
            "path_rename" => self.path_rename(mem, args),
            "path_create_directory" => self.path_op(mem, a(0), a(1), a(2), Tree::mkdir),
            "path_remove_directory" => self.path_op(mem, a(0), a(1), a(2), Tree::rmdir),
            "path_filestat_get" => self.path_filestat_get(mem, a(0), a(2), a(3), a(4)),
            "fd_read" => self.fd_read(mem, a(0), a(1), a(2), a(3)),
            "fd_write" => self.fd_write(mem, a(0), a(1), a(2), a(3)),
            "fd_close" => self.fd_close(a(0)),
            "fd_seek" => self.fd_seek(mem, a(0), args[1] as i64, a(2), a(3)),
            "fd_filestat_get" => self.fd_filestat_get(mem, a(0), a(1)),
            "clock_time_get" => clock_time_get(mem, a(0), a(2)),
            "random_get" => Err(Errno::NOSYS),
            _ => return Err(Stop::BadCall),
        };
        Ok(match result {
            Ok(()) => 0,
            Err(errno) => errno.0,
        })
    }

    fn args_sizes_get(&self, mem: &mut Guest, argc: u32, size: u32) -> Res {
        mem.bytes(argc, 4)?;
        mem.bytes(size, 4)?;
        mem.put_u32(argc, self.argc)?;
        mem.put_u32(size, self.args.len() as u32)
    }

    fn args_get(&self, mem: &mut Guest, argv: u32, buf: u32) -> Res {
        mem.bytes(argv, self.argc.checked_mul(4).ok_or(Errno::FAULT)?)?;
        mem.put(buf, &self.args)?;
        let mut offset = 0u32;
        for (i, arg) in self.args.split_inclusive(|&b| b == 0).enumerate() {
            let at = buf.checked_add(offset).ok_or(Errno::FAULT)?;
            mem.put_u32(argv + 4 * i as u32, at)?;
            offset += arg.len() as u32;
        }
        Ok(())
    }

    fn fd(&self, fd: u32) -> Result<&Fd, Errno> {
        self.fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(Errno::BADF)
    }

    fn preopen(&self, fd: u32) -> Result<&'static str, Errno> {
        let index = (fd as usize).checked_sub(3).ok_or(Errno::BADF)?;
        let name = PREOPENS.get(index).ok_or(Errno::BADF)?;
        match self.fd(fd)? {
            Fd::Dir(path) if path == name => Ok(name),
            _ => Err(Errno::BADF),
        }
    }

    fn fd_prestat_get(&self, mem: &mut Guest, fd: u32, out: u32) -> Res {
        let name = self.preopen(fd)?;
        let mut prestat = [0u8; 8];
        prestat[4..].copy_from_slice(&(name.len() as u32).to_le_bytes());
        mem.put(out, &prestat)
    }

    fn fd_prestat_dir_name(&self, mem: &mut Guest, fd: u32, out: u32, len: u32) -> Res {
        let name = self.preopen(fd)?;
        if (len as usize) < name.len() {
            return Err(Errno::NAMETOOLONG);
        }
        mem.put(out, name.as_bytes())
    }

    /// The absolute path a directory fd and a guest path name.
    fn resolve(&self, mem: &Guest, dir: u32, path: u32, len: u32) -> Result<String, Errno> {
        let Fd::Dir(base) = self.fd(dir)? else {
            return Err(Errno::NOTDIR);
        };
        if len as usize > MAX_PATH {
            return Err(Errno::NAMETOOLONG);
        }
        tree::join(base, mem.bytes(path, len)?)
    }

    fn path_op(
        &mut self,
        mem: &mut Guest,
        dir: u32,
        path: u32,
        len: u32,
        op: fn(&mut Tree, &str) -> Res,
    ) -> Res {
        let full = self.resolve(mem, dir, path, len)?;
        op(&mut self.tree, &full)
    }

    fn path_rename(&mut self, mem: &mut Guest, args: &[u64]) -> Res {
        let a = |i: usize| args[i] as u32;
        let from = self.resolve(mem, a(0), a(1), a(2))?;
        let to = self.resolve(mem, a(3), a(4), a(5))?;
        self.tree.rename(&from, &to)
    }

    fn path_open(&mut self, mem: &mut Guest, args: &[u64]) -> Res {
        let a = |i: usize| args[i] as u32;
        let (oflags, rights, fdflags, out) = (a(4), args[5], a(7), a(8));
        mem.bytes(out, 4)?;
        let path = self.resolve(mem, a(0), a(2), a(3))?;
        let write = rights & RIGHT_FD_WRITE != 0;
        let fd = match self.tree.kind(&path) {
            Some(Kind::Dir) if oflags & OFLAGS_TRUNC != 0 || write => return Err(Errno::ISDIR),
            Some(Kind::Dir) => Fd::Dir(path),
            Some(Kind::File) if oflags & OFLAGS_DIRECTORY != 0 => return Err(Errno::NOTDIR),
            Some(Kind::File) if oflags & OFLAGS_CREAT != 0 && oflags & OFLAGS_EXCL != 0 => {
                return Err(Errno::EXIST);
            }
            found => {
                if write && !tree::writable(&path) {
                    return Err(Errno::ROFS);
                }
                if found.is_none() {
                    if oflags & OFLAGS_CREAT == 0 {
                        return Err(Errno::NOENT);
                    }
                    self.free_fd()?;
                    self.tree.create(&path)?;
                } else if oflags & OFLAGS_TRUNC != 0 {
                    self.tree.truncate(&path)?;
                }
                Fd::File {
                    path,
                    pos: 0,
                    read: rights & RIGHT_FD_READ != 0,
                    write,
                    append: fdflags & FDFLAGS_APPEND != 0,
                }
            }
        };
        let number = self.free_fd()?;
        if number == self.fds.len() {
            self.fds.push(Some(fd));
        } else {
            self.fds[number] = Some(fd);
        }
        mem.put_u32(out, number as u32)
    }

    /// The lowest free fd number, or `MFILE` at the limit.
    fn free_fd(&self) -> Result<usize, Errno> {
        let number = self
            .fds
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.fds.len());
        if number >= self.limits.fds {
            return Err(Errno::MFILE);
        }
        Ok(number)
    }

    fn path_filestat_get(&self, mem: &mut Guest, dir: u32, path: u32, len: u32, out: u32) -> Res {
        mem.bytes(out, 64)?;
        let path = self.resolve(mem, dir, path, len)?;
        let stat = match self.tree.kind(&path).ok_or(Errno::NOENT)? {
            Kind::Dir => filestat(FILETYPE_DIRECTORY, 0),
            Kind::File => filestat(FILETYPE_REGULAR_FILE, self.tree.file(&path)?.len() as u64),
        };
        mem.put(out, &stat)
    }

    fn fd_filestat_get(&self, mem: &mut Guest, fd: u32, out: u32) -> Res {
        let stat = match self.fd(fd)? {
            Fd::Stdin | Fd::Out(_) => filestat(FILETYPE_CHARACTER_DEVICE, 0),
            Fd::Dir(_) => filestat(FILETYPE_DIRECTORY, 0),
            Fd::File { path, .. } => {
                filestat(FILETYPE_REGULAR_FILE, self.tree.file(path)?.len() as u64)
            }
        };
        mem.put(out, &stat)
    }

    fn fd_read(&mut self, mem: &mut Guest, fd: u32, iovs: u32, count: u32, out: u32) -> Res {
        mem.bytes(out, 4)?;
        let iovs = mem.iovs(iovs, count)?;
        let (path, pos) = match self.fd(fd)? {
            Fd::Stdin => return mem.put_u32(out, 0),
            Fd::File {
                path,
                pos,
                read: true,
                ..
            } => (path.clone(), *pos),
            Fd::Dir(_) => return Err(Errno::ISDIR),
            _ => return Err(Errno::BADF),
        };
        let data = self.tree.file(&path)?;
        let mut at = usize::try_from(pos).unwrap_or(usize::MAX).min(data.len());
        let mut total = 0u32;
        for (ptr, len) in iovs {
            let take = (len as usize).min(data.len() - at);
            mem.put(ptr, &data[at..at + take])?;
            at += take;
            total += take as u32;
        }
        if let Some(Some(Fd::File { pos, .. })) = self.fds.get_mut(fd as usize) {
            *pos = at as u64;
        }
        mem.put_u32(out, total)
    }

    fn fd_write(&mut self, mem: &mut Guest, fd: u32, iovs: u32, count: u32, out: u32) -> Res {
        mem.bytes(out, 4)?;
        let iovs = mem.iovs(iovs, count)?;
        let mut data = Vec::new();
        for (ptr, len) in iovs {
            data.extend_from_slice(mem.bytes(ptr, len)?);
        }
        let total = u32::try_from(data.len()).map_err(|_| Errno::INVAL)?;
        match self.fd(fd)?.clone() {
            Fd::Out(n) => self.capture(n, &data),
            Fd::File {
                path,
                pos,
                write: true,
                append,
                ..
            } => {
                let at = if append {
                    self.tree.file(&path)?.len() as u64
                } else {
                    pos
                };
                self.tree.write_at(&path, at, &data)?;
                if let Some(Some(Fd::File { pos, .. })) = self.fds.get_mut(fd as usize) {
                    *pos = at + u64::from(total);
                }
            }
            Fd::Dir(_) => return Err(Errno::ISDIR),
            _ => return Err(Errno::BADF),
        }
        mem.put_u32(out, total)
    }

    fn capture(&mut self, fd: u32, data: &[u8]) {
        let limit = self.limits.output_bytes;
        let sink = if fd == FD_STDOUT {
            &mut self.stdout
        } else {
            debug_assert_eq!(fd, FD_STDERR);
            &mut self.stderr
        };
        let room = limit.saturating_sub(sink.len());
        let take = room.min(data.len());
        sink.extend_from_slice(&data[..take]);
        if take < data.len() {
            self.truncated = true;
        }
    }

    fn fd_close(&mut self, fd: u32) -> Res {
        self.fd(fd)?;
        self.fds[fd as usize] = None;
        Ok(())
    }

    fn fd_seek(&mut self, mem: &mut Guest, fd: u32, offset: i64, whence: u32, out: u32) -> Res {
        mem.bytes(out, 8)?;
        let (path, pos) = match self.fd(fd)? {
            Fd::File { path, pos, .. } => (path, *pos),
            Fd::Stdin | Fd::Out(_) => return Err(Errno::SPIPE),
            Fd::Dir(_) => return Err(Errno::BADF),
        };
        let base = match whence {
            0 => 0,
            1 => pos,
            2 => self.tree.file(path)?.len() as u64,
            _ => return Err(Errno::INVAL),
        };
        let new = i64::try_from(base)
            .ok()
            .and_then(|b| b.checked_add(offset))
            .filter(|n| *n >= 0)
            .ok_or(Errno::INVAL)? as u64;
        if let Some(Some(Fd::File { pos, .. })) = self.fds.get_mut(fd as usize) {
            *pos = new;
        }
        mem.put_u64(out, new)
    }
}

fn environ_sizes_get(mem: &mut Guest, count: u32, size: u32) -> Res {
    mem.bytes(count, 4)?;
    mem.bytes(size, 4)?;
    mem.put_u32(count, 0)?;
    mem.put_u32(size, 0)
}

fn clock_time_get(mem: &mut Guest, id: u32, out: u32) -> Res {
    if id > MAX_CLOCK_ID {
        return Err(Errno::INVAL);
    }
    mem.put_u64(out, 0)
}

/// Preview 1's 64-byte `filestat`; every time is 0 and every file has one link.
fn filestat(filetype: u8, size: u64) -> [u8; 64] {
    let mut stat = [0u8; 64];
    stat[16] = filetype;
    stat[24..32].copy_from_slice(&1u64.to_le_bytes());
    stat[32..40].copy_from_slice(&size.to_le_bytes());
    stat
}
