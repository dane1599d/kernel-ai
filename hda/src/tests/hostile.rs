//! The codec and the controller are devices: what they say is never trusted.
//! Seeded garbage in every answer and in the RIRB, extreme graphs, every wait
//! stuck, and a controller that reads all ones. Each ends in Ok or a named
//! Err, never a panic or a hang (the fake clock bounds every wait), and no
//! DMA is left running after an error.

use super::*;
use crate::controller::VERB_BUDGET;

/// The rings and the stream are stopped and point at nothing.
fn quiet(regs: &FakeRegs) {
    assert_eq!(regs.read8(CORBCTL) & RING_RUN, 0, "the CORB stopped");
    assert_eq!(regs.read8(RIRBCTL) & RING_RUN, 0, "the RIRB stopped");
    for base in [CORBLBASE, CORBUBASE, RIRBLBASE, RIRBUBASE] {
        assert_eq!(regs.read32(base), 0, "ring base {base:#x} cleared");
    }
    assert_eq!(regs.read8(sd(4, SD_CTL0)) & SD_RUN, 0, "the stream stopped");
    assert_eq!(regs.read32(sd(4, SD_BDPL)), 0);
    assert_eq!(regs.read32(sd(4, SD_BDPU)), 0);
}

#[test]
fn random_answers_from_the_codec() {
    for seed in 1..=300u64 {
        let mut codec = qemu_duplex();
        codec.chaos = Some(seed * 0x9E37_79B9);
        let regs = FakeRegs::new(codec);
        let got = Controller::new(FakeHost::default(), &regs).play_tone();
        assert!(regs.state().commands <= VERB_BUDGET as usize, "seed {seed}");
        if got.is_err() {
            quiet(&regs);
        }
    }
}

#[test]
fn garbage_in_the_rirb() {
    // Random entries anywhere, answers from other codecs and unsolicited
    // ones, and write pointers from anywhere, all ones included.
    for seed in 1..=300u64 {
        let regs = FakeRegs::new(qemu_duplex());
        regs.state().hostile_rirb = Some(seed * 0x2545_F491);
        let got = Controller::new(FakeHost::default(), &regs).play_tone();
        assert!(regs.state().commands <= VERB_BUDGET as usize, "seed {seed}");
        if got.is_err() {
            quiet(&regs);
        }
    }
}

#[test]
fn an_all_ones_codec_has_no_audio_group() {
    // A codec answering all ones, as an unplugged one might: a function
    // group past 255 nodes, of no audio type.
    let codec = FakeCodec {
        chaos: Some(0),
        ..FakeCodec::default()
    };
    let regs = FakeRegs::new(codec);
    struct Ones;
    impl Codec for Ones {
        fn verb(&mut self, _: u8, _: Verb) -> Result<u32, Error> {
            Ok(u32::MAX)
        }
    }
    assert_eq!(
        graph::audio_function_group(&mut Ones),
        Err(Error::NoAudioFunctionGroup)
    );
    let _ = Controller::new(FakeHost::default(), &regs).play_tone();
}

/// A 255-node group in which every node is a connected output pin listing 32
/// others and none reaches a DAC: the walk would send millions of commands,
/// and the budget ends it.
#[test]
fn a_255_node_maze_meets_the_budget() {
    let mut c = FakeCodec::default();
    c.node(0).params.extend([
        (param::VENDOR_ID, 0x1AF4_0022),
        (param::NODE_COUNT, 0x0001_0001),
    ]);
    c.node(1).params.extend([
        (param::FUNCTION_GROUP_TYPE, 0x01),
        (param::NODE_COUNT, 0x0001_00FF),
    ]);
    for nid in 2..=255u8 {
        let n = c.widget(nid, PIN | CONN_LIST);
        n.params.insert(param::PIN_CAP, u32::MAX);
        n.config = LINE_OUT;
        // Long form: node numbers past 127 would end ranges in the short one.
        n.long = true;
        n.conns = (0..32)
            .map(|k| 2 + (u16::from(nid) + k * 7) % 254)
            .collect();
    }
    let regs = FakeRegs::new(c);
    let mut hda = Controller::new(FakeHost::default(), &regs);
    assert_eq!(
        hda.play_tone(),
        Err(Error::Codec("over 4096 commands in one bring-up"))
    );
    assert_eq!(regs.state().commands, VERB_BUDGET as usize);
    quiet(&regs);
    // The budget is the bring-up's: a second one starts afresh.
    let regs = FakeRegs::new(qemu_duplex());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    hda.play_tone().unwrap();
    hda.stop().unwrap();
    hda.play_tone().unwrap();
}

#[test]
fn extreme_widget_words_on_every_node() {
    // Every capability bit set on every node, or every type in turn.
    for caps in [u32::MAX, 0x00F0_0000, 0x0040_0000 | 0x200, 0x0020_01FF, 0] {
        let mut c = FakeCodec::default();
        c.node(0).params.insert(param::NODE_COUNT, 0x0001_0001);
        c.node(1).params.extend([
            (param::FUNCTION_GROUP_TYPE, 0x01),
            (param::NODE_COUNT, 0x0002_00FE),
        ]);
        for nid in 2..=255u8 {
            let n = c.widget(nid, caps);
            n.params.insert(param::PIN_CAP, u32::MAX);
            n.config = u32::MAX;
            n.conns = vec![u16::from(nid.wrapping_add(1)), 0x80 | u16::from(nid)];
        }
        let regs = FakeRegs::new(c);
        let got = Controller::new(FakeHost::default(), &regs).play_tone();
        assert!(
            got.is_err() || caps == 0x0040_0000 | 0x200,
            "{caps:#x}: {got:?}"
        );
        assert!(regs.state().commands <= VERB_BUDGET as usize);
    }
}

#[test]
fn connection_lists_past_32_are_refused_unread() {
    for long in [false, true] {
        for length in 33..=127u32 {
            let mut c = FakeCodec::default();
            c.node(9).length = Some(length | if long { 0x80 } else { 0 });
            assert_eq!(
                graph::connections(&mut c, 9),
                Err(Error::Codec("a connection list longer than 32")),
                "{length} long={long}"
            );
            assert_eq!(c.list_reads, 0, "no word of it was read");
        }
    }
    // 32 short entries take 8 reads, one per word, not 32.
    let mut c = FakeCodec::default();
    c.node(9).conns = (2..34).collect();
    assert_eq!(graph::connections(&mut c, 9).unwrap().as_slice().len(), 32);
    assert_eq!(c.list_reads, 8);
    // And 32 long ones, 16.
    let n = c.node(10);
    n.long = true;
    n.conns = (2..34).collect();
    graph::connections(&mut c, 10).unwrap();
    assert_eq!(c.list_reads, 8 + 16);
}

#[test]
fn every_stuck_wait_is_its_own_timeout() {
    let cases = [
        (Stuck::RingsRun, "the rings to stop"),
        (Stuck::InReset, "the link to enter reset"),
        (Stuck::OutOfReset, "the link to leave reset"),
        (Stuck::CorbRp, "the CORB read pointer to leave reset"),
        (Stuck::RingsIdle, "the rings to run"),
        (Stuck::StreamReset, "the stream to leave reset"),
        (Stuck::StreamIdle, "the stream to run"),
    ];
    for (stuck, what) in cases {
        let regs = FakeRegs::new(qemu_duplex());
        {
            let mut s = regs.state();
            s.stuck = Some(stuck);
            match stuck {
                Stuck::RingsRun => s.put(CORBCTL, 1, u64::from(RING_RUN)),
                Stuck::InReset => s.put(GCTL, 4, 1),
                _ => {}
            }
        }
        let got = Controller::new(FakeHost::default(), &regs).play_tone();
        assert_eq!(got, Err(Error::Timeout(what)), "{stuck:?}");
        if !matches!(stuck, Stuck::RingsRun | Stuck::StreamReset) {
            quiet(&regs);
        }
    }
    // A stream that will not stop: stop() says so, and still stops the rings.
    let regs = FakeRegs::new(qemu_duplex());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    hda.play_tone().unwrap();
    regs.state().stuck = Some(Stuck::StreamRuns);
    assert_eq!(hda.stop(), Err(Error::Timeout("the stream to stop")));
    assert_eq!(regs.read8(CORBCTL) & RING_RUN, 0);
    assert_eq!(regs.read32(sd(4, SD_BDPL)), 0);
}

#[test]
fn a_codec_that_wakes_late_is_waited_for() {
    // STATESTS shows nothing for 50 reads (well under 100 ms), then codec 0.
    let regs = FakeRegs::new(qemu_duplex());
    regs.state().wake_reads = 50;
    Controller::new(FakeHost::default(), &regs)
        .play_tone()
        .unwrap();
    // Never: no codec, not a timeout.
    let regs = FakeRegs::new(qemu_duplex());
    regs.state().wake_reads = u32::MAX;
    assert_eq!(
        Controller::new(FakeHost::default(), &regs).play_tone(),
        Err(Error::NoCodec)
    );
}

#[test]
fn an_unanswered_command_stops_the_rings() {
    let mut silent = qemu_duplex();
    silent.silent = true;
    let regs = FakeRegs::new(silent);
    let mut hda = Controller::new(FakeHost::default(), &regs);
    assert!(matches!(hda.play_tone(), Err(Error::NoResponse { .. })));
    quiet(&regs);
    // A late answer can never be taken for the next command's.
    assert_eq!(
        hda.command(0, Verb::get_parameter(0)),
        Err(Error::Unsupported("the rings are not running"))
    );
}

/// A controller that reads all ones (absent, or unplugged mid-way).
struct AllOnes;

impl Regs for AllOnes {
    fn read8(&self, _: u64) -> u8 {
        u8::MAX
    }
    fn read16(&self, _: u64) -> u16 {
        u16::MAX
    }
    fn read32(&self, _: u64) -> u32 {
        u32::MAX
    }
    fn write8(&self, _: u64, _: u8) {}
    fn write16(&self, _: u64, _: u16) {}
    fn write32(&self, _: u64, _: u32) {}
}

#[test]
fn an_all_ones_controller_times_out_at_once() {
    let mut hda = Controller::new(FakeHost::default(), AllOnes);
    assert_eq!(hda.play_tone(), Err(Error::Timeout("the rings to stop")));
    assert_eq!(hda.position(), None);
    assert_eq!(hda.stop(), Ok(()), "nothing was started");
}

/// The one teardown, in order: the stream's RUN off, its reset, its
/// registers zeroed, then the rings stopped and their bases zeroed.
#[test]
fn the_teardown_order() {
    let regs = FakeRegs::new(qemu_duplex());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    hda.play_tone().unwrap();
    regs.state().writes.clear();
    hda.stop().unwrap();
    let s = |reg| sd(4, reg);
    assert_eq!(
        regs.state().writes,
        [
            (s(SD_CTL0), 0),
            (s(SD_CTL0), u64::from(SD_SRST)),
            (s(SD_CTL0), 0),
            (s(SD_BDPL), 0),
            (s(SD_BDPU), 0),
            (s(SD_CBL), 0),
            (s(SD_LVI), 0),
            (CORBCTL, 0),
            (RIRBCTL, 0),
            (CORBLBASE, 0),
            (CORBUBASE, 0),
            (RIRBLBASE, 0),
            (RIRBUBASE, 0),
        ]
    );
    quiet(&regs);
    assert_eq!(hda.position(), None);
    // A stream that never ran is torn down the same way: the descriptor was
    // programmed before the RUN wait.
    let regs = FakeRegs::new(qemu_duplex());
    regs.state().stuck = Some(Stuck::StreamIdle);
    let mut hda = Controller::new(FakeHost::default(), &regs);
    assert_eq!(hda.play_tone(), Err(Error::Timeout("the stream to run")));
    let writes = regs.state().writes.clone();
    let after_run = writes
        .iter()
        .rposition(|&(at, v)| at == sd(4, SD_CTL0) && v & u64::from(SD_RUN) != 0)
        .unwrap();
    assert!(writes[after_run..].contains(&(sd(4, SD_BDPL), 0)));
    assert!(writes[after_run..].contains(&(CORBLBASE, 0)));
    quiet(&regs);
}
