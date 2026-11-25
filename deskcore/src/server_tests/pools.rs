//! Pools `desktop` lets go: a client knows whether a pool is in use, and one
//! let go takes its buffers with it, so nothing is drawn from, or made on, a
//! pool whose mapping is gone.

use super::*;

#[test]
fn a_pool_is_in_use_while_a_buffer_of_it_lives() {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(1, "app");
    assert!(!client.pool_in_use(2));
    client.handle(&mut desk, &setup(), true).unwrap();
    assert!(client.pool_in_use(2));
    client
        .handle(&mut desk, &bytes(&[(4, Request::BufferDestroy)]), false)
        .unwrap();
    assert!(!client.pool_in_use(2), "its only buffer is destroyed");
}

#[test]
fn a_pool_let_go_takes_its_buffers_and_refuses_a_commit_on_them() {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(1, "app");
    client.handle(&mut desk, &setup(), true).unwrap();
    client.pool_mapped(2);
    client.pool_gone(2);
    assert!(!client.pool_in_use(2));
    assert!(client.buffer(4).is_none());
    // A buffer made on the pool, or a commit of the one it had, is refused.
    let late = bytes(&[(
        2,
        Request::PoolBuffer {
            id: 9,
            offset: 4096,
            width: 64,
            height: 32,
            stride: 256,
            format: Format::Xrgb8888,
        },
    )]);
    assert!(client.handle(&mut desk, &late, false).is_err());
    let mut client = Client::new(1, "app");
    client.handle(&mut desk, &setup(), true).unwrap();
    client.pool_gone(2);
    assert!(client.handle(&mut desk, &show(), false).is_err());
}

#[test]
fn letting_go_of_one_pool_leaves_the_others() {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(1, "app");
    client.handle(&mut desk, &setup(), true).unwrap();
    client.pool_mapped(2);
    client.pool_gone(99);
    assert!(client.pool_in_use(2));
    assert!(client.buffer(4).is_some());
}
