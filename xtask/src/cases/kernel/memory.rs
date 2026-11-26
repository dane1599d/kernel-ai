//! K's memory cases: resource profiles, shared pages and scriptrun's Job.

use super::{TestCase, case};

pub(in crate::cases) const CASES: &[TestCase] = &[
    // P3 task 12: resource profiles' kernel half.
    TestCase {
        expect: &[
            "profile: a quota stopped a commit, a class showed in SCHED_STATS, and the floors held",
            // Bug hunt 5 H5-17: a lowered quota still counts what is held past it.
            "profile: a quota lowered under what its process holds freed no admission budget",
        ],
        ..case("profile", "test=profile")
    },
    // P1 task 14: shared pages, and spike S01-1's M6.
    TestCase {
        expect: &[
            "shm: M6: revoke a 256-page Shm",
            // #1155's review: a refund lands only on the pid it was charged to.
            "shm: a refund for another pid leaves a slot's count; one for its own pid comes off by its pages",
            "shm: a failed SHM_MAP took back the pages it had mapped and shot them down",
            // Bug hunt 5 H5-16: SHM_MAP's page tables are counted, and go with SHM_UNMAP.
            "shm: SHM_UNMAP gave back the page tables SHM_MAP added, and a map past the quota was refused",
            "shm: SHM_CREATE, MAP and UNMAP work, and a revoke leaves holders sharing nothing",
        ],
        ..case("shm", "test=shm")
    },
    // P1 task 14: scriptrun through a Job, and spike S01-1's M3 and M5.
    TestCase {
        expect: &[
            "handles: M3: scriptrun per request",
            "handles: M3: scriptrun pooled",
            "handles: a killed pooled child left the pool, and the next claim got a new one",
            "scriptrun: M5: ",
            "scriptrun: a Job started scriptrun with only the handles moved in, and its exit came back",
        ],
        ..case("scriptrun", "test=scriptrun")
    },
];
