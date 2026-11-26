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

/// H3-27: a window's surface and a11y end after its toplevel, the surface's
/// record (and tree) with them; a surface still a window's is refused; the
/// ids are free again and the old a11y is no object of the client's.
#[test]
fn a_windows_surface_and_a11y_end_after_its_toplevel() {
    let mut desk = Desk::new(1280, 800);
    let open = |desk: &mut Desk, pid| {
        let mut client = Client::new(pid, "app");
        client.handle(desk, &setup(), true).unwrap();
        client.pool_mapped(2);
        let a11y = bytes(&[(3, Request::GetA11y { id: 6 })]);
        client.handle(desk, &a11y, false).unwrap();
        client.handle(desk, &show(), false).unwrap();
        client
    };
    let mut early = open(&mut desk, 1);
    let refused = early.handle(&mut desk, &bytes(&[(3, Request::SurfaceDestroy)]), false);
    assert!(matches!(refused, Err(Error::BadArg)), "{refused:?}");
    let mut client = open(&mut desk, 2);
    let gone = bytes(&[
        (5, Request::ToplevelDestroy),
        (4, Request::BufferDestroy),
        (6, Request::A11yDestroy),
        (3, Request::SurfaceDestroy),
        (2, Request::PoolDestroy),
    ]);
    let needs = client.handle(&mut desk, &gone, false).unwrap();
    assert!(matches!(needs[..], [Need::Closed { .. }]), "{needs:?}");
    client.pool_gone(2);
    let surface = Request::Bind {
        name: 2,
        interface: "surface",
        version: 1,
        id: 3,
    };
    assert!(
        client
            .handle(&mut desk, &bytes(&[(REGISTRY, surface)]), false)
            .is_ok()
    );
    let late = client.handle(&mut desk, &bytes(&[(6, Request::A11yCommit)]), false);
    assert!(matches!(late, Err(Error::UnknownObject)), "{late:?}");
}
