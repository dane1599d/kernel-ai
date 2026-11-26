//! Archives listed, extracted and made: what goes in comes out, folders
//! before what is in them, and `Maker` keeps an opened archive's rules.

mod kit;

use kaizip::{Archive, Error, Kind, Limits, Maker, Tree};
use kit::Raw;
use std::io::Cursor;

const MIB: usize = 1 << 20;

fn open(bytes: &[u8]) -> Result<Archive<Cursor<&[u8]>>, Error> {
    Archive::open(Cursor::new(bytes), Limits::default())
}

/// Every byte of `bytes` from a sequence that does not repeat soon.
fn noise(len: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9u32;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// Makes an archive of `files` and `folders` with `limits`.
fn make(folders: &[&str], files: &[(&str, &[u8])], limits: Limits) -> Result<Vec<u8>, Error> {
    let mut maker = Maker::new(Cursor::new(Vec::new()), limits);
    for folder in folders {
        maker.folder(folder)?;
    }
    for (path, bytes) in files {
        maker.file(path, &mut &bytes[..])?;
    }
    Ok(maker.finish()?.into_inner())
}

#[test]
fn what_is_made_lists_and_extracts_as_it_went_in() {
    let big = noise(3 * MIB + 7);
    let text = b"a page of text, ".repeat(10_000);
    let files: [(&str, &[u8]); 4] = [
        ("Documents/notes.txt", &text),
        ("Documents/deep/er/still/data.bin", &big),
        ("empty", b""),
        ("caf\u{e9} \u{1F600}.txt", "unicode names".as_bytes()),
    ];
    let bytes = make(&["Pictures/", "Documents"], &files, Limits::default()).unwrap();
    // The same files make the same bytes.
    assert_eq!(
        bytes,
        make(&["Pictures/", "Documents"], &files, Limits::default()).unwrap()
    );
    let mut archive = open(&bytes).unwrap();
    let listed: Vec<(&str, Kind, u64)> = archive
        .entries()
        .iter()
        .map(|e| (e.path.as_str(), e.kind, e.size))
        .collect();
    assert_eq!(
        listed,
        [
            ("Pictures", Kind::Folder, 0),
            ("Documents", Kind::Folder, 0),
            ("Documents/notes.txt", Kind::File, text.len() as u64),
            (
                "Documents/deep/er/still/data.bin",
                Kind::File,
                big.len() as u64
            ),
            ("empty", Kind::File, 0),
            ("caf\u{e9} \u{1F600}.txt", Kind::File, 13),
        ]
    );
    // Text packs; noise does not.
    assert!(archive.entries()[2].packed < text.len() as u64 / 20);
    let mut tree = Tree::default();
    archive.extract(&mut tree).unwrap();
    for (path, bytes) in files {
        assert!(tree.files[path] == bytes, "{path}");
    }
    // The folders the names imply are made too.
    let folders: Vec<&str> = tree.folders.iter().map(String::as_str).collect();
    assert_eq!(
        folders,
        [
            "Documents",
            "Documents/deep",
            "Documents/deep/er",
            "Documents/deep/er/still",
            "Pictures"
        ]
    );
}

/// Each folder reaches the `Unpack` before anything in it, even when the
/// archive names a file first and never names its folders.
#[test]
fn folders_come_before_what_is_in_them() {
    struct Log(Vec<String>);
    impl kaizip::Unpack for Log {
        fn folder(&mut self, path: &str) -> Result<(), String> {
            self.0.push(format!("folder {path}"));
            Ok(())
        }
        fn begin(&mut self, path: &str, size: u64) -> Result<(), String> {
            self.0.push(format!("file {path} {size}"));
            Ok(())
        }
        fn write(&mut self, _: &[u8]) -> Result<(), String> {
            Ok(())
        }
        fn end(&mut self) -> Result<(), String> {
            self.0.push("end".into());
            Ok(())
        }
    }
    let bytes = kit::archive(&[
        Raw::stored("a/b/c.txt", b"one"),
        Raw::folder("a/"),
        Raw::deflated("a/d.txt", b"two two two two"),
    ]);
    let mut log = Log(Vec::new());
    open(&bytes).unwrap().extract(&mut log).unwrap();
    assert_eq!(
        log.0,
        [
            "folder a",
            "folder a/b",
            "file a/b/c.txt 3",
            "end",
            "file a/d.txt 15",
            "end"
        ]
    );
    // An Unpack's error stops it.
    struct Full;
    impl kaizip::Unpack for Full {
        fn folder(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn begin(&mut self, _: &str, _: u64) -> Result<(), String> {
            Err("the disk is full".into())
        }
        fn write(&mut self, _: &[u8]) -> Result<(), String> {
            Ok(())
        }
        fn end(&mut self) -> Result<(), String> {
            Ok(())
        }
    }
    let got = open(&bytes).unwrap().extract(&mut Full);
    assert_eq!(got, Err(Error::Io("the disk is full".into())));
}

#[test]
fn a_maker_keeps_the_rules_an_archive_is_opened_with() {
    let limits = Limits::default();
    for name in [
        "../x", "/abs", "C:/x", "a\\b", "a//b", "./a", "a/../b", "a\nb", "",
    ] {
        let got = make(&[], &[(name, b"x")], limits);
        assert!(matches!(got, Err(Error::Name { .. })), "{name:?}: {got:?}");
    }
    assert!(matches!(
        make(&[], &[("dir/", b"x")], limits),
        Err(Error::Name { .. })
    ));
    for (folders, files) in [
        (&[][..], &[("a", &b"1"[..]), ("a", b"2")][..]),
        (&[], &[("a", b"1"), ("a/b", b"2")]),
        (&[], &[("a/b", b"1"), ("a", b"2")]),
        (&["a"], &[("a", b"1")]),
    ] {
        let got = make(folders, files, limits);
        assert!(
            matches!(got, Err(Error::Duplicate { .. })),
            "{files:?}: {got:?}"
        );
    }
    // A folder named twice, or after a file in it, is one folder.
    assert!(make(&["a", "a/"], &[("a/b", b"1")], limits).is_ok());
    let few = Limits {
        max_entries: 2,
        ..limits
    };
    assert!(matches!(
        make(&["a", "b", "c"], &[], few),
        Err(Error::TooManyEntries { count: 3, max: 2 })
    ));
    let small = Limits {
        max_entry: 10,
        max_total: 15,
        ..limits
    };
    assert!(matches!(
        make(&[], &[("a", &[0; 11])], small),
        Err(Error::TooLarge { .. })
    ));
    assert!(matches!(
        make(&[], &[("a", &[0; 8]), ("b", &[0; 8])], small),
        Err(Error::TooLarge { .. })
    ));
    assert!(make(&[], &[("a", &[0; 8]), ("b", &[0; 7])], small).is_ok());
}

#[test]
fn the_refusals_say_what_and_why_in_words() {
    let encrypted = Error::Encrypted {
        name: "secret.txt".into(),
    };
    assert_eq!(
        encrypted.to_string(),
        "secret.txt is encrypted; password-protected zip files are not opened"
    );
    let name = Error::Name {
        name: "../etc/passwd".into(),
        why: "it has a . or .. part",
    };
    assert_eq!(
        name.to_string(),
        "the name \"../etc/passwd\" is refused: it has a . or .. part"
    );
    assert!(Error::Overlap.to_string().contains("zip bomb"));
}
