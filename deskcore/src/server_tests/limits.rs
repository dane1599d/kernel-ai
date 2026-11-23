//! What a client that never commits or never reads may make `desktop` keep:
//! its pending damage folds into one rectangle past a header page's worth, and
//! its queued events stop at a cap and flag it for dropping.

use super::*;
use crate::server::MAX_EVENT_BYTES;

#[test]
fn damage_that_never_commits_does_not_grow_without_end() {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(1, "raw");
    client.handle(&mut desk, &setup(), true).unwrap();
    client.pool_mapped(2);
    // Hundreds of rectangles, call after call, and no commit.
    let one = |n: u32| {
        bytes(&[(
            3,
            Request::DamageBuffer(SRect {
                x: n % 60,
                y: n % 30,
                w: 2,
                h: 2,
            }),
        )])
    };
    let mut call = Vec::new();
    for n in 0..300 {
        call.extend(one(n));
    }
    for _ in 0..20 {
        client.handle(&mut desk, &call, false).unwrap();
    }
    // The one commit that follows shows the whole of it as a rectangle or few.
    let commit = bytes(&[(3, Request::Attach { buffer: 4 }), (3, Request::Commit)]);
    client.handle(&mut desk, &commit, false).unwrap();
    assert!(desk.take_damage().len() <= 40);
}

#[test]
fn a_client_whose_events_pile_up_is_flagged_and_its_queue_stops_growing() {
    let mut client = Client::new(1, "slow");
    assert!(!client.too_far_behind());
    for _ in 0..100_000 {
        client.theme("light");
    }
    assert!(client.too_far_behind());
    // What it was sent is capped near the limit, not by how many were sent.
    let kept = client.take_events(usize::MAX).len();
    assert!(kept < MAX_EVENT_BYTES + 256, "{kept}");
    // Reading the queue leaves it able to hear again.
    assert!(!client.too_far_behind());
    client.theme("dark");
    assert!(!client.take_events(8192).is_empty());
}
