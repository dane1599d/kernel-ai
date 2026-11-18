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
    // A restart finds them and adds nothing.
    let generation = c.server.generation();
    drop(c);
    let (mut c, _) = Client::start(&mut disk);
    assert_eq!(c.server.generation(), generation);
}

/// `memory-volume`'s two boots on the host: the probe's purge drops the two
/// snapshots that held its record, and after a restart none holds it.
#[test]
fn the_memory_probe_purges_and_a_restart_finds_no_snapshot_holding_it() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    assert_eq!(c.server.memory_probe(), Ok(2));
    drop(c);
    let (mut c, _) = Client::start(&mut disk);
    assert_eq!(c.server.memory_check(), Ok((1, 0)));
    // A client's open action keeps the probe out.
    c.home(Request::Begin { action: 1 });
    assert_eq!(c.server.memory_probe(), Err(Error::Busy));
}
