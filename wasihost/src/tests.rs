use super::*;
use alloc::vec;

const MEM: usize = 4096;
/// Where tests put paths, iovecs, data and results in guest memory.
const PATH: u32 = 0x100;
const IOV: u32 = 0x200;
const DATA: u32 = 0x300;
const OUT: u32 = 0x800;

fn wasi() -> Wasi {
    let mut tree = Tree::default();
    tree.add_kai("include/kai.h", b"int x;\n".to_vec()).unwrap();
    Wasi::new(tree, &["wcc", "-c", "x.c"], Limits::default()).unwrap()
}

fn put(mem: &mut [u8], at: u32, bytes: &[u8]) {
    mem[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
}

fn u32_at(mem: &[u8], at: u32) -> u32 {
    u32::from_le_bytes(mem[at as usize..at as usize + 4].try_into().unwrap())
}

fn u64_at(mem: &[u8], at: u32) -> u64 {
    u64::from_le_bytes(mem[at as usize..at as usize + 8].try_into().unwrap())
}

fn open(w: &mut Wasi, mem: &mut [u8], dir: u64, path: &str, oflags: u64, rights: u64) -> u16 {
    put(mem, PATH, path.as_bytes());
    let args = [
        dir,
        0,
        PATH as u64,
        path.len() as u64,
        oflags,
        rights,
        rights,
        0,
        OUT as u64,
    ];
    w.call("path_open", mem, &args).unwrap()
}

/// Writes `data` through one iovec; returns the errno.
fn write(w: &mut Wasi, mem: &mut [u8], fd: u64, data: &[u8]) -> u16 {
    put(mem, DATA, data);
    put(mem, IOV, &DATA.to_le_bytes());
    put(mem, IOV + 4, &(data.len() as u32).to_le_bytes());
    w.call("fd_write", mem, &[fd, IOV as u64, 1, OUT as u64])
        .unwrap()
}

fn read(w: &mut Wasi, mem: &mut [u8], fd: u64, len: u32) -> (u16, Vec<u8>) {
    put(mem, IOV, &DATA.to_le_bytes());
    put(mem, IOV + 4, &len.to_le_bytes());
    let errno = w
        .call("fd_read", mem, &[fd, IOV as u64, 1, OUT as u64])
        .unwrap();
    let n = u32_at(mem, OUT) as usize;
    (errno, mem[DATA as usize..DATA as usize + n].to_vec())
}

const RW: u64 = RIGHT_FD_READ | RIGHT_FD_WRITE;
const CREAT: u64 = OFLAGS_CREAT as u64;

#[test]
fn twenty_imports_with_unique_names() {
    assert_eq!(IMPORTS.len(), 20);
    for (i, a) in IMPORTS.iter().enumerate() {
        assert!(
            IMPORTS[i + 1..].iter().all(|b| b.name != a.name),
            "{}",
            a.name
        );
    }
}

#[test]
fn unknown_call_and_wrong_arity_stop() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    assert_eq!(w.call("sock_accept", &mut mem, &[]), Err(Stop::BadCall));
    assert_eq!(w.call("fd_close", &mut mem, &[1, 2]), Err(Stop::BadCall));
}

#[test]
fn args_round_trip() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    assert_eq!(
        w.call("args_sizes_get", &mut mem, &[OUT as u64, OUT as u64 + 4]),
        Ok(0)
    );
    assert_eq!(u32_at(&mem, OUT), 3);
    assert_eq!(u32_at(&mem, OUT + 4), 11);
    assert_eq!(
        w.call("args_get", &mut mem, &[IOV as u64, DATA as u64]),
        Ok(0)
    );
    assert_eq!(u32_at(&mem, IOV), DATA);
    assert_eq!(u32_at(&mem, IOV + 4), DATA + 4);
    assert_eq!(u32_at(&mem, IOV + 8), DATA + 7);
    assert_eq!(&mem[DATA as usize..DATA as usize + 11], b"wcc\0-c\0x.c\0");
}

#[test]
fn preopens_are_kai_and_w() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    for (fd, name) in [(3u64, "/kai"), (4, "/w"), (5, "/tmp")] {
        assert_eq!(w.call("fd_prestat_get", &mut mem, &[fd, OUT as u64]), Ok(0));
        assert_eq!(mem[OUT as usize], 0);
        assert_eq!(u32_at(&mem, OUT + 4) as usize, name.len());
        let args = [fd, DATA as u64, name.len() as u64];
        assert_eq!(w.call("fd_prestat_dir_name", &mut mem, &args), Ok(0));
        assert_eq!(
            &mem[DATA as usize..DATA as usize + name.len()],
            name.as_bytes()
        );
    }
    assert_eq!(
        w.call("fd_prestat_get", &mut mem, &[6, OUT as u64]),
        Ok(Errno::BADF.0)
    );
    let short = [3, DATA as u64, 2];
    assert_eq!(
        w.call("fd_prestat_dir_name", &mut mem, &short),
        Ok(Errno::NAMETOOLONG.0)
    );
}

#[test]
fn reads_a_kai_file() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    assert_eq!(
        open(&mut w, &mut mem, 3, "include/kai.h", 0, RIGHT_FD_READ),
        0
    );
    let fd = u32_at(&mem, OUT) as u64;
    assert_eq!(fd, 6);
    assert_eq!(read(&mut w, &mut mem, fd, 100), (0, b"int x;\n".to_vec()));
    assert_eq!(read(&mut w, &mut mem, fd, 100), (0, vec![]));
}

#[test]
fn kai_is_read_only() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    assert_eq!(
        open(&mut w, &mut mem, 3, "include/kai.h", 0, RW),
        Errno::ROFS.0
    );
    assert_eq!(open(&mut w, &mut mem, 3, "new.h", CREAT, RW), Errno::ROFS.0);
    put(&mut mem, PATH, b"include/kai.h");
    let args = [3, PATH as u64, 13];
    assert_eq!(
        w.call("path_unlink_file", &mut mem, &args),
        Ok(Errno::ROFS.0)
    );
    let args = [3, PATH as u64, 7];
    assert_eq!(
        w.call("path_remove_directory", &mut mem, &args),
        Ok(Errno::ROFS.0)
    );
    assert_eq!(w.tree().read("/kai/include/kai.h"), Some(&b"int x;\n"[..]));
}

#[test]
fn writes_and_reads_back_scratch() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    assert_eq!(open(&mut w, &mut mem, 4, "entry.o", CREAT, RW), 0);
    let fd = u32_at(&mem, OUT) as u64;
    assert_eq!(write(&mut w, &mut mem, fd, b"\0asm"), 0);
    assert_eq!(u32_at(&mem, OUT), 4);
    assert_eq!(w.call("fd_seek", &mut mem, &[fd, 0, 0, OUT as u64]), Ok(0));
    assert_eq!(read(&mut w, &mut mem, fd, 2), (0, b"\0a".to_vec()));
    assert_eq!(
        w.call("fd_seek", &mut mem, &[fd, -1i64 as u64, 2, OUT as u64]),
        Ok(0)
    );
    assert_eq!(u64_at(&mem, OUT), 3);
    assert_eq!(w.call("fd_close", &mut mem, &[fd]), Ok(0));
    assert_eq!(w.call("fd_close", &mut mem, &[fd]), Ok(Errno::BADF.0));
    assert_eq!(w.tree().read("/w/entry.o"), Some(&b"\0asm"[..]));
    assert_eq!(w.tree().scratch_used(), 4);
}

#[test]
fn seek_past_end_zero_fills_and_negative_seek_is_refused() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    open(&mut w, &mut mem, 4, "a", CREAT, RW);
    let fd = u32_at(&mem, OUT) as u64;
    assert_eq!(w.call("fd_seek", &mut mem, &[fd, 3, 0, OUT as u64]), Ok(0));
    write(&mut w, &mut mem, fd, b"z");
    assert_eq!(w.tree().read("/w/a"), Some(&b"\0\0\0z"[..]));
    let back = [fd, -10i64 as u64, 1, OUT as u64];
    assert_eq!(w.call("fd_seek", &mut mem, &back), Ok(Errno::INVAL.0));
    assert_eq!(
        w.call("fd_seek", &mut mem, &[1, 0, 0, OUT as u64]),
        Ok(Errno::SPIPE.0)
    );
}

#[test]
fn creat_excl_trunc_and_append() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    open(&mut w, &mut mem, 4, "f", CREAT, RW);
    let fd = u32_at(&mem, OUT) as u64;
    write(&mut w, &mut mem, fd, b"hello");
    let excl = CREAT | OFLAGS_EXCL as u64;
    assert_eq!(open(&mut w, &mut mem, 4, "f", excl, RW), Errno::EXIST.0);
    assert_eq!(
        open(&mut w, &mut mem, 4, "missing", 0, RIGHT_FD_READ),
        Errno::NOENT.0
    );
    put(&mut mem, PATH, b"f");
    let args = [
        4,
        0,
        PATH as u64,
        1,
        0,
        RW,
        RW,
        FDFLAGS_APPEND as u64,
        OUT as u64,
    ];
    assert_eq!(w.call("path_open", &mut mem, &args), Ok(0));
    let app = u32_at(&mem, OUT) as u64;
    write(&mut w, &mut mem, app, b"!");
    assert_eq!(w.tree().read("/w/f"), Some(&b"hello!"[..]));
    assert_eq!(open(&mut w, &mut mem, 4, "f", OFLAGS_TRUNC as u64, RW), 0);
    assert_eq!(w.tree().read("/w/f"), Some(&b""[..]));
    assert_eq!(w.tree().scratch_used(), 0);
}

#[test]
fn directories_rename_and_unlink() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    put(&mut mem, PATH, b"d");
    assert_eq!(
        w.call("path_create_directory", &mut mem, &[4, PATH as u64, 1]),
        Ok(0)
    );
    assert_eq!(
        w.call("path_create_directory", &mut mem, &[4, PATH as u64, 1]),
        Ok(20)
    );
    open(&mut w, &mut mem, 4, "d/x", CREAT, RW);
    put(&mut mem, PATH, b"d");
    let rm = [4, PATH as u64, 1];
    assert_eq!(
        w.call("path_remove_directory", &mut mem, &rm),
        Ok(Errno::NOTEMPTY.0)
    );
    put(&mut mem, PATH, b"d/x");
    put(&mut mem, DATA, b"y");
    let mv = [4, PATH as u64, 3, 4, DATA as u64, 1];
    assert_eq!(w.call("path_rename", &mut mem, &mv), Ok(0));
    assert_eq!(w.tree().kind("/w/y"), Some(Kind::File));
    assert_eq!(w.tree().kind("/w/d/x"), None);
    put(&mut mem, PATH, b"d");
    assert_eq!(w.call("path_remove_directory", &mut mem, &rm), Ok(0));
    put(&mut mem, PATH, b"y");
    assert_eq!(
        w.call("path_unlink_file", &mut mem, &[4, PATH as u64, 1]),
        Ok(0)
    );
    assert_eq!(
        open(&mut w, &mut mem, 4, "nodir/x", CREAT, RW),
        Errno::NOENT.0
    );
}

#[test]
fn escapes_are_refused() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    for path in [
        "../kai/include/kai.h",
        "/kai/include/kai.h",
        "a/../../x",
        "a\0b",
    ] {
        assert_eq!(
            open(&mut w, &mut mem, 4, path, CREAT, RW),
            Errno::NOTCAPABLE.0,
            "{path}"
        );
    }
    assert_eq!(open(&mut w, &mut mem, 1, "x", CREAT, RW), Errno::NOTDIR.0);
    assert_eq!(open(&mut w, &mut mem, 99, "x", CREAT, RW), Errno::BADF.0);
    put(&mut mem, PATH, &[0xff, 0xfe]);
    let args = [4, 0, PATH as u64, 2, 0, 0, 0, 0, OUT as u64];
    assert_eq!(w.call("path_open", &mut mem, &args), Ok(Errno::INVAL.0));
}

#[test]
fn stdout_and_stderr_are_captured_and_capped() {
    let mut mem = vec![0u8; MEM];
    let limits = Limits {
        fds: 8,
        output_bytes: 4,
    };
    let mut w = Wasi::new(Tree::default(), &["wcc"], limits).unwrap();
    assert_eq!(write(&mut w, &mut mem, 1, b"abc"), 0);
    assert_eq!(write(&mut w, &mut mem, 1, b"def"), 0);
    assert_eq!(u32_at(&mem, OUT), 3);
    assert_eq!(write(&mut w, &mut mem, 2, b"err"), 0);
    assert_eq!(w.stdout(), b"abcd");
    assert_eq!(w.stderr(), b"err");
    assert!(w.output_truncated());
    assert_eq!(read(&mut w, &mut mem, 0, 10), (0, vec![]));
}

#[test]
fn clock_is_zero_random_is_refused_exit_stops() {
    let mut mem = vec![0xAAu8; MEM];
    let mut w = wasi();
    assert_eq!(
        w.call("clock_time_get", &mut mem, &[0, 1, OUT as u64]),
        Ok(0)
    );
    assert_eq!(u64_at(&mem, OUT), 0);
    assert_eq!(
        w.call("clock_time_get", &mut mem, &[9, 1, OUT as u64]),
        Ok(28)
    );
    assert_eq!(w.call("random_get", &mut mem, &[DATA as u64, 8]), Ok(52));
    assert_eq!(mem[DATA as usize], 0xAA);
    assert_eq!(w.call("proc_exit", &mut mem, &[3]), Err(Stop::Exit(3)));
    assert_eq!(w.exit_code(), Some(3));
}

#[test]
fn filestat_reports_type_and_size() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    put(&mut mem, PATH, b"include/kai.h");
    let args = [3, 0, PATH as u64, 13, OUT as u64];
    assert_eq!(w.call("path_filestat_get", &mut mem, &args), Ok(0));
    assert_eq!(mem[OUT as usize + 16], FILETYPE_REGULAR_FILE);
    assert_eq!(u64_at(&mem, OUT + 32), 7);
    let args = [3, 0, PATH as u64, 7, OUT as u64];
    assert_eq!(w.call("path_filestat_get", &mut mem, &args), Ok(0));
    assert_eq!(mem[OUT as usize + 16], FILETYPE_DIRECTORY);
    assert_eq!(w.call("fd_filestat_get", &mut mem, &[1, OUT as u64]), Ok(0));
    assert_eq!(mem[OUT as usize + 16], FILETYPE_CHARACTER_DEVICE);
}

#[test]
fn every_pointer_out_of_memory_faults() {
    let mut mem = vec![0u8; 64];
    let mut w = wasi();
    let far = u32::MAX as u64;
    assert_eq!(w.call("args_sizes_get", &mut mem, &[61, 0]), Ok(21));
    assert_eq!(w.call("args_get", &mut mem, &[0, 60]), Ok(21));
    assert_eq!(w.call("fd_prestat_get", &mut mem, &[3, 60]), Ok(21));
    assert_eq!(w.call("fd_write", &mut mem, &[1, far, 1, 0]), Ok(21));
    assert_eq!(w.call("fd_write", &mut mem, &[1, 0, 1, far]), Ok(21));
    // An iovec whose buffer runs off the end.
    mem[0..8].copy_from_slice(&[32, 0, 0, 0, 64, 0, 0, 0]);
    assert_eq!(w.call("fd_write", &mut mem, &[1, 0, 1, 40]), Ok(21));
    assert_eq!(
        w.call("fd_write", &mut mem, &[1, 0, MAX_IOVS as u64 + 1, 40]),
        Ok(28)
    );
    assert_eq!(w.call("clock_time_get", &mut mem, &[0, 0, 60]), Ok(21));
    assert_eq!(w.stdout(), b"");
}

#[test]
fn scratch_and_fd_limits_hold() {
    let mut mem = vec![0u8; MEM];
    let tree = Tree::new(TreeLimits {
        scratch_bytes: 8,
        scratch_entries: 2,
    });
    let limits = Limits {
        fds: 7,
        output_bytes: 16,
    };
    let mut w = Wasi::new(tree, &["wcc"], limits).unwrap();
    assert_eq!(open(&mut w, &mut mem, 4, "a", CREAT, RW), 0);
    let fd = u32_at(&mem, OUT) as u64;
    assert_eq!(write(&mut w, &mut mem, fd, b"12345678"), 0);
    assert_eq!(write(&mut w, &mut mem, fd, b"9"), Errno::NOSPC.0);
    assert_eq!(
        open(&mut w, &mut mem, 4, "a", 0, RIGHT_FD_READ),
        Errno::MFILE.0
    );
    w.call("fd_close", &mut mem, &[fd]).unwrap();
    assert_eq!(open(&mut w, &mut mem, 4, "b", CREAT, RW), 0);
    w.call("fd_close", &mut mem, &[6]).unwrap();
    assert_eq!(open(&mut w, &mut mem, 4, "c", CREAT, RW), Errno::NOSPC.0);
    w.tree_mut().clear_scratch();
    assert_eq!(w.tree().scratch_used(), 0);
    assert_eq!(open(&mut w, &mut mem, 4, "c", CREAT, RW), 0);
}

#[test]
fn bad_args_are_refused() {
    assert!(Wasi::new(Tree::default(), &["a\0b"], Limits::default()).is_err());
    let long = "x".repeat(MAX_ARGS_BYTES);
    assert!(Wasi::new(Tree::default(), &[&long], Limits::default()).is_err());
}

#[test]
fn unlinked_file_lives_until_its_last_fd_closes() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    assert_eq!(open(&mut w, &mut mem, 5, "tmpXYZ", CREAT, RW), 0);
    let fd = u32_at(&mem, OUT) as u64;
    put(&mut mem, PATH, b"tmpXYZ");
    assert_eq!(
        w.call("path_unlink_file", &mut mem, &[5, PATH as u64, 6]),
        Ok(0)
    );
    assert_eq!(w.tree().kind("/tmp/tmpXYZ"), None);
    assert_eq!(write(&mut w, &mut mem, fd, b"#line 1"), 0);
    assert_eq!(w.call("fd_seek", &mut mem, &[fd, 0, 0, OUT as u64]), Ok(0));
    assert_eq!(read(&mut w, &mut mem, fd, 64), (0, b"#line 1".to_vec()));
    assert_eq!(w.tree().scratch_used(), 7);
    assert_eq!(w.call("fd_close", &mut mem, &[fd]), Ok(0));
    assert_eq!(w.tree().scratch_used(), 0);
}

#[test]
fn clear_scratch_keeps_kai() {
    let mut mem = vec![0u8; MEM];
    let mut w = wasi();
    open(&mut w, &mut mem, 4, "entry.o", CREAT, RW);
    let mut tree = w.into_tree();
    tree.clear_scratch();
    assert_eq!(tree.kind("/w/entry.o"), None);
    assert_eq!(tree.kind("/w"), Some(Kind::Dir));
    assert_eq!(tree.read("/kai/include/kai.h"), Some(&b"int x;\n"[..]));
    tree.put_scratch("entry.c", b"int y;").unwrap();
    assert_eq!(tree.read("/w/entry.c"), Some(&b"int y;"[..]));
    tree.put_scratch("entry.c", b"z").unwrap();
    assert_eq!(tree.read("/w/entry.c"), Some(&b"z"[..]));
    assert_eq!(tree.scratch_used(), 1);
}
