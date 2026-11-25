//! ADR 0013's `memory` and `index` volumes through `fs`: a disk gains both at
//! start, once, and no badge reaches either, so no app route does.

use super::tests::{ALICE, BLOCKS, Client};
use super::*;
use kaifs::MemDisk;
use kaifs::vol::{AGENT_VOLUMES, INDEX, MEMORY};

#[test]
fn a_disk_gains_memory_and_index_at_start_and_no_badge_reaches_them() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let volumes = c.server.fs().volumes();
    for (id, name) in AGENT_VOLUMES {
        assert!(volumes.contains(&(id, name.into())), "{name}");
    }
    let file = c.open("a.txt", How::CreateFile).unwrap();
    let real = Badge::parse(file.badge);
    for volume in [MEMORY, INDEX] {
        for file in [false, true] {
            let badge = Badge {
                volume: volume as u8,
                file,
                object: kaifs::ROOT,
                ..real
            };
            let reply = c.call(ALICE, badge.raw(), DIR_RIGHTS, Request::Stat).0;
            assert_eq!(reply, Reply::Failed(Error::NotFound), "volume {volume}");
        }
    }
    // A restart finds them and adds nothing. (Its commit of the skipped id
    // counters, #1137, moves the generation, so the volumes are compared.)
    drop(c);
    let (mut c, _) = Client::start(&mut disk);
    assert_eq!(c.server.fs().volumes(), volumes);
}
