//! The allocator's reserve (audit, HIGH): growth stops while the reserve is
//! still free, so the work that frees space, which needs new blocks for its
//! own commit, can always run on a disk growth has filled.

use kaifs::check::{Cache, check};
use kaifs::{Error, Fs, MemDisk, ROOT};

#[test]
fn growth_stops_at_the_reserve_and_freeing_work_can_still_commit() {
    let mut fs = Fs::mkfs(MemDisk::new(2048), [1; 16], "full").unwrap();
    fs.commit().unwrap();
    let reserve = fs.io.alloc.reserve();
    // Fill with 4 KiB files, a commit each, until growth is refused.
    let mut made = 0;
    loop {
        let grown = fs
            .create(ROOT, &format!("f{made}"))
            .and_then(|f| fs.write_file(f, &[7; 4096]))
            .and_then(|()| fs.commit().map(|_| ()));
        match grown {
            Ok(()) => made += 1,
            Err(Error::NoSpace) => {
                fs.abort().unwrap();
                break;
            }
            Err(e) => panic!("{e:?}"),
        }
        assert!(made < 10_000);
    }
    let (full, _) = fs.space();
    assert!(
        full >= reserve,
        "{full} free, under the reserve of {reserve}"
    );
    // Removing half takes the reserve for its commit, then two commits give
    // the freed blocks back.
    fs.freeing(|fs| {
        for n in 0..made / 2 {
            fs.remove(ROOT, &format!("f{n}")).unwrap();
        }
        for _ in 0..3 {
            fs.commit().unwrap();
        }
    });
    let (freed, _) = fs.space();
    assert!(freed > full + made / 4, "{full} free full, {freed} after");
    // Growth works again, and the filesystem checks.
    let f = fs.create(ROOT, "again").unwrap();
    fs.write_file(f, &[1; 4096]).unwrap();
    fs.commit().unwrap();
    check(&mut fs, &mut Cache::default()).unwrap();
}
