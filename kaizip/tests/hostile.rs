//! Hostile archives, written byte by byte: every one is refused whole, with
//! why, before a byte of it is extracted, or stopped as it is extracted.

#[path = "../../testkit/hostile.rs"]
mod hostile;
#[path = "hostile/zip.rs"]
mod zip;

mod kit;

use kaizip::{Archive, Error, Limits, Tree};
use kit::Raw;
use std::io::Cursor;

const MIB: u64 = 1 << 20;

fn open_with(bytes: &[u8], limits: Limits) -> Result<Tree, Error> {
    let mut archive = Archive::open(Cursor::new(bytes), limits)?;
    let mut tree = Tree::default();
    archive.extract(&mut tree)?;
    Ok(tree)
}

fn open(bytes: &[u8]) -> Result<Tree, Error> {
    open_with(bytes, Limits::default())
}

#[test]
fn names_that_could_escape_or_are_not_plain_are_refused() {
    let long = "n".repeat(kaizip::names::MAX_NAME + 1);
    let part = format!("a/{}", "p".repeat(kaizip::names::MAX_PART + 1));
    let deep = vec!["d"; 33].join("/");
    for name in [
        "../escape",
        "a/../../escape",
        "a/..",
        "..",
        "/etc/passwd",
        "//server/share",
        "C:/Windows/x",
        "C:x",
        "c:\\x",
        "a\\..\\..\\x",
        "\\x",
        "file.txt:stream",
        "a//b",
        "./a",
        "a/./b",
        "a\0b",
        "a\nb",
        "a\u{7f}b",
        "",
        long.as_str(),
        part.as_str(),
        deep.as_str(),
    ] {
        let got = open(&kit::archive(&[
            Raw::stored("ok.txt", b"ok"),
            Raw::stored(name, b"x"),
        ]));
        assert!(matches!(got, Err(Error::Name { .. })), "{name:?}: {got:?}");
    }
    // 32 deep is allowed.
    let deep = vec!["d"; 32].join("/");
    assert!(open(&kit::archive(&[Raw::stored(&deep, b"x")])).is_ok());
    // A name that is not UTF-8 is read as CP437, as zip has it, and checked.
    let mut cp437 = Raw::stored("x", b"x");
    cp437.name = b"caf\x82.txt".to_vec();
    let tree = open(&kit::archive(&[cp437])).unwrap();
    assert_eq!(tree.files.keys().next().unwrap(), "caf\u{e9}.txt");
}

#[test]
fn links_devices_and_encrypted_entries_are_refused_by_name() {
    for mode in [0o120_777, 0o010_644, 0o020_644, 0o060_644, 0o140_644] {
        let entry = Raw {
            mode: Some(mode),
            ..Raw::stored("link", b"/etc/passwd")
        };
        let got = open(&kit::archive(&[entry]));
        assert_eq!(
            got,
            Err(Error::Special {
                name: "link".into()
            }),
            "{mode:o}"
        );
    }
    // ZipCrypto's flag, and AES (method 99) with it.
    for method in [0, 8, 99] {
        let entry = Raw {
            flags: 1,
            method,
            ..Raw::stored("secret.txt", b"xxxxxxxxxxxx")
        };
        let got = open(&kit::archive(&[Raw::stored("a", b"a"), entry]));
        assert_eq!(
            got,
            Err(Error::Encrypted {
                name: "secret.txt".into()
            }),
            "{method}"
        );
    }
    // Named before its name is checked, a hostile name is shown escaped: no
    // new line in the message, no bidi control turning it round (#1447's
    // review).
    let entry = Raw {
        flags: 1,
        ..Raw::stored("x", b"xxxxxxxxxxxx")
    };
    let entry = Raw {
        name: "secret\n\u{202e}txt.exe\u{7}".as_bytes().to_vec(),
        ..entry
    };
    let got = open(&kit::archive(&[entry]));
    let shown = r"secret\n\u{202e}txt.exe\u{7}";
    assert_eq!(got, Err(Error::Encrypted { name: shown.into() }));
    let message = got.unwrap_err().to_string();
    assert!(
        !message.contains('\n') && !message.contains('\u{202e}'),
        "{message}"
    );
    let link = Raw {
        mode: Some(0o120_777),
        name: b"a\x1b[2Jb".to_vec(),
        ..Raw::stored("x", b"/etc/passwd")
    };
    assert_eq!(
        open(&kit::archive(&[link])),
        Err(Error::Special {
            name: r"a\u{1b}[2Jb".into()
        })
    );
    // Methods other than stored and deflate: bzip2, LZMA, zstd, deflate64.
    for method in [12, 14, 93, 9] {
        let entry = Raw {
            method,
            ..Raw::stored("packed.bin", b"data")
        };
        let got = open(&kit::archive(&[entry]));
        assert!(
            matches!(got, Err(Error::Method { .. })),
            "{method}: {got:?}"
        );
    }
}

#[test]
fn duplicate_names_and_files_with_entries_under_them_are_refused() {
    for entries in [
        vec![Raw::stored("a.txt", b"1"), Raw::stored("a.txt", b"2")],
        vec![Raw::stored("a", b"1"), Raw::stored("a/b", b"2")],
        vec![Raw::stored("a/b", b"1"), Raw::stored("a", b"2")],
        vec![Raw::folder("a/"), Raw::stored("a", b"2")],
        vec![Raw::stored("a", b"1"), Raw::folder("a/")],
    ] {
        let got = open(&kit::archive(&entries));
        assert!(matches!(got, Err(Error::Duplicate { .. })), "{got:?}");
    }
    // Names differ by case: two files, as on a case-sensitive disk.
    let tree = open(&kit::archive(&[
        Raw::stored("A", b"1"),
        Raw::stored("a", b"2"),
    ]))
    .unwrap();
    assert_eq!(tree.files.len(), 2);
}

#[test]
fn bombs_are_refused_by_count_size_ratio_and_shared_data() {
    let entries: Vec<Raw> = (0..6)
        .map(|n| Raw::stored(&format!("f{n}"), b"x"))
        .collect();
    let few = Limits {
        max_entries: 5,
        ..Limits::default()
    };
    assert!(matches!(
        open_with(&kit::archive(&entries), few),
        Err(Error::TooManyEntries { count: 6, max: 5 })
    ));
    // Sizes as the headers say them, before anything is extracted.
    let says = |size: u64| Raw {
        size: size as u32,
        ..Raw::stored("big", b"x")
    };
    // Any ratio here: these say sizes their one byte cannot hold.
    let limits = Limits {
        max_entry: 10 * MIB,
        max_total: 15 * MIB,
        max_ratio: u64::MAX,
        ..Limits::default()
    };
    assert!(matches!(
        open_with(&kit::archive(&[says(11 * MIB)]), limits),
        Err(Error::TooLarge { .. })
    ));
    let two = [
        says(8 * MIB),
        Raw {
            name: b"big2".to_vec(),
            ..says(8 * MIB)
        },
    ];
    assert!(matches!(
        open_with(&kit::archive(&two), limits),
        Err(Error::TooLarge { .. })
    ));
    // 16 MiB of zeros deflates about a thousand times.
    let zeros = vec![0u8; 16 * MIB as usize];
    let bomb = Raw::deflated("zeros", &zeros);
    assert!(bomb.data.len() < 40_000, "{}", bomb.data.len());
    assert_eq!(
        open(&kit::archive(&[bomb])),
        Err(Error::Ratio {
            name: "zeros".into()
        })
    );
    // Under the floor, any ratio.
    let small = Raw::deflated("zeros", &zeros[..MIB as usize]);
    assert_eq!(
        open(&kit::archive(&[small])).unwrap().files["zeros"].len(),
        MIB as usize
    );
    // One compressed block made many files: the overlapping-file bomb.
    let first = Raw::deflated("one", &vec![7u8; 100_000]);
    let second = Raw {
        name: b"two".to_vec(),
        same_data_as: Some(0),
        ..first.clone()
    };
    assert_eq!(open(&kit::archive(&[first, second])), Err(Error::Overlap));
}

/// An entry whose bytes are not what its header says stops extraction.
#[test]
fn an_entry_that_lies_about_its_size_or_crc_is_damaged() {
    let data = b"0123456789".repeat(10);
    let says_less = Raw {
        size: 10,
        ..Raw::deflated("a", &data)
    };
    let says_more = Raw {
        size: 1000,
        ..Raw::deflated("a", &data)
    };
    let bad_crc = Raw {
        crc: 7,
        ..Raw::stored("a", &data)
    };
    for entry in [says_less, says_more, bad_crc] {
        let got = open(&kit::archive(&[entry]));
        assert!(matches!(got, Err(Error::Damaged { .. })), "{got:?}");
    }
    // Real bytes past max_total stop it, whatever the headers say.
    let limits = Limits {
        max_total: 150,
        ..Limits::default()
    };
    let one = Raw::stored("a", &data);
    let two = Raw::stored("b", &data);
    // Each says 100; 200 is past 150, so it is refused on what they say.
    assert!(matches!(
        open_with(&kit::archive(&[one, two]), limits),
        Err(Error::TooLarge { .. })
    ));
}

#[test]
fn what_is_not_a_zip_is_refused() {
    let good = kit::archive(&[Raw::stored("a", b"hello")]);
    for bytes in [
        &b""[..],
        b"not a zip at all",
        &good[..good.len() - 5],
        &good[4..],
        b"PK\x05\x06",
    ] {
        let got = open(bytes);
        assert!(matches!(got, Err(Error::NotZip(_))), "{got:?}");
    }
    // An empty archive is an empty folder.
    let tree = open(&kit::archive(&[])).unwrap();
    assert!(tree.files.is_empty() && tree.folders.is_empty());
}

/// What the `zip_open` and `zip_extract` bodies may hold at once: no more
/// than the largest file [`zip::LIMITS`] lets through (117 KiB measured),
/// since extraction streams and the body keeps no file's bytes.
const ZIP_PEAK: usize = 1 << 20;

/// The `zip_open` and `zip_extract` fuzz bodies (P6 G27) on mutations and
/// truncations of good archives, bombs and names that escape: each returns,
/// keeps the ceilings, extracts only under its root, and holds no more than
/// [`ZIP_PEAK`] at once.
#[test]
fn the_zip_bodies_keep_the_ceilings_and_the_root_on_hostile_archives() {
    use kaizip::Maker;
    let mut maker = Maker::new(Cursor::new(Vec::new()), Limits::default());
    maker.folder("docs").unwrap();
    maker
        .file(
            "docs/a.txt",
            &mut Cursor::new(b"some text, some text".repeat(20)),
        )
        .unwrap();
    maker
        .file("b.bin", &mut Cursor::new([0u8, 1, 2, 255]))
        .unwrap();
    let made = maker.finish().unwrap().into_inner();
    let raw = kit::archive(&[
        Raw::folder("x/"),
        Raw::stored("x/y.txt", b"stored bytes"),
        Raw::deflated("x/z/w.txt", &b"deflated ".repeat(50)),
    ]);
    // A file near the small limits' largest, which streams out in pieces.
    let mut rng = hostile::XorShift::new(0x27);
    let noise: Vec<u8> = (0..900 << 10).map(|_| rng.next() as u8).collect();
    let large = kit::archive(&[Raw::stored("noise.bin", &noise)]);
    // A deflate bomb past the small limits' ratio and size.
    let bomb = kit::archive(&[Raw::deflated("zeros", &vec![0; 3 << 20])]);
    // Under their size, past their ratio: refused by the ratio alone.
    let zeros = vec![0u8; 1 << 20];
    let ratio = kit::archive(&[Raw::deflated("zeros", &zeros)]);
    // A bomb that says it is small: it opens, under the ratio's floor, and
    // is stopped as it is extracted, past the size it said. Its pair says
    // more than it holds, and is stopped short.
    let lying = kit::archive(&[
        Raw {
            size: 1000,
            ..Raw::deflated("says-less", &zeros[..200 << 10])
        },
        Raw {
            size: 5000,
            ..Raw::deflated("says-more", &zeros[..100])
        },
    ]);
    let says_more = kit::archive(&[Raw {
        size: 5000,
        ..Raw::deflated("says-more", &zeros[..100])
    }]);
    // A central directory said to start past the end of the archive.
    let mut past_end = raw.clone();
    let eocd = past_end.len() - 22;
    assert_eq!(&past_end[eocd..eocd + 4], b"PK\x05\x06");
    let at = (past_end.len() as u32 + 100).to_le_bytes();
    past_end[eocd + 16..eocd + 20].copy_from_slice(&at);
    let opened = |data: &[u8]| Archive::open(Cursor::new(data), zip::LIMITS).map(|_| ());
    assert!(matches!(opened(&ratio), Err(Error::Ratio { .. })));
    assert!(matches!(opened(&past_end), Err(Error::NotZip(_))));
    for liar in [&lying, &says_more] {
        let mut archive = Archive::open(Cursor::new(&liar[..]), zip::LIMITS).unwrap();
        let got = archive.extract(&mut Tree::default());
        assert!(matches!(got, Err(Error::Damaged { .. })), "{got:?}");
    }
    let escapes = kit::archive(&[
        Raw::stored("../up", b"x"),
        Raw::stored("a/../../up", b"x"),
        Raw::stored("/abs", b"x"),
        Raw::stored("c:\\win", b"x"),
    ]);
    for good in [&made, &raw, &large] {
        assert!(zip::zip_open(good).is_some(), "a good archive opens");
    }
    for refused in [&bomb, &escapes, &ratio, &past_end] {
        assert!(
            zip::zip_open(refused).is_none(),
            "a hostile archive is refused"
        );
    }
    let mut most = 0;
    let mut run = |data: &[u8]| {
        let ((), peak) = hostile::peak_alloc(|| {
            zip::zip_open(data);
            zip::zip_extract(data);
        });
        assert!(peak <= ZIP_PEAK, "{peak} bytes at once");
        most = most.max(peak);
    };
    let cases = if cfg!(debug_assertions) { 1_000 } else { 5_000 };
    for (seed, sample) in [
        (0x27_0001, &made),
        (0x27_0002, &raw),
        (0x27_0003, &bomb),
        (0x27_0004, &escapes),
        (0x27_0006, &ratio),
        (0x27_0007, &lying),
        (0x27_0008, &past_end),
    ] {
        run(sample);
        hostile::fuzz(sample, seed, cases, &mut run);
        hostile::truncations(sample, |_, bytes| run(bytes));
    }
    // The large file a few times only: each case reads all of it.
    run(&large);
    hostile::fuzz(&large, 0x27_0005, 50, &mut run);
    run(&[]);
    eprintln!("zip bodies: at most {most} bytes at once");
}
