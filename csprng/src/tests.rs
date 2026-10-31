use super::*;

/// ChaCha20's keystream for the all-zero key and nonce, block 0 (the
/// well-known test vector, RFC 7539 appendix A.1).
const ZERO_KEY_BLOCK0: [u8; 32] = [
    0x76, 0xb8, 0xe0, 0xad, 0xa0, 0xf1, 0x3d, 0x90, 0x40, 0x5d, 0x6a, 0xe5, 0x53, 0x86, 0xbd, 0x28,
    0xbd, 0xd2, 0x19, 0xb8, 0xa0, 0x8d, 0xed, 0x1a, 0xa8, 0x36, 0xef, 0xcc, 0x8b, 0x77, 0x0d, 0xc7,
];

#[test]
fn the_first_fill_is_chacha20s_keystream() {
    let mut rng = Csprng::new([0; SEED_BYTES], 0);
    let mut out = [0u8; 32];
    assert!(rng.fill(&mut out));
    assert_eq!(out, ZERO_KEY_BLOCK0);
}

#[test]
fn the_same_seed_gives_the_same_bytes_and_another_seed_does_not() {
    let fills = |seed: [u8; SEED_BYTES]| {
        let mut rng = Csprng::new(seed, 0);
        let mut out = [[0u8; 64]; 3];
        for o in &mut out {
            assert!(rng.fill(o));
        }
        out
    };
    assert_eq!(fills([7; SEED_BYTES]), fills([7; SEED_BYTES]));
    assert_ne!(fills([7; SEED_BYTES]), fills([8; SEED_BYTES]));
}

#[test]
fn each_fill_moves_to_a_new_key() {
    // Without erasure, the second fill would be the stream's bytes 32..64.
    let mut rng = Csprng::new([0; SEED_BYTES], 0);
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    assert!(rng.fill(&mut a));
    assert!(rng.fill(&mut b));
    let mut plain = ChaCha20Rng::from_seed([0; SEED_BYTES]);
    let mut stream = [0u8; 64];
    plain.fill_bytes(&mut stream);
    assert_eq!(a, stream[..32]);
    // The next key is the stream's bytes 32..64, so they never reach a caller.
    assert_ne!(b, stream[32..]);
    let mut next = ChaCha20Rng::from_seed(stream[32..].try_into().unwrap());
    let mut expected = [0u8; 32];
    next.fill_bytes(&mut expected);
    assert_eq!(b, expected);
}

#[test]
fn a_request_over_the_limit_fills_nothing() {
    let mut rng = Csprng::new([1; SEED_BYTES], 0);
    let mut big = [0u8; MAX_REQUEST + 1];
    assert!(!rng.fill(&mut big));
    assert!(big.iter().all(|&b| b == 0));
    let mut most = [0u8; MAX_REQUEST];
    assert!(rng.fill(&mut most));
    assert!(rng.fill(&mut []));
}

#[test]
fn it_reseeds_after_enough_bytes_or_time() {
    let mut rng = Csprng::new([2; SEED_BYTES], 1_000);
    assert!(!rng.needs_reseed(1_000));
    assert!(!rng.needs_reseed(1_000 + RESEED_MS - 1));
    assert!(rng.needs_reseed(1_000 + RESEED_MS));
    let mut out = [0u8; MAX_REQUEST];
    for _ in 0..RESEED_BYTES / MAX_REQUEST as u64 {
        assert!(rng.fill(&mut out));
    }
    assert!(rng.needs_reseed(1_000));
    rng.reseed([3; SEED_BYTES], 2_000);
    assert!(!rng.needs_reseed(2_000));
    // A failed hardware read waits a whole period before the next try.
    assert!(rng.needs_reseed(2_000 + RESEED_MS));
    rng.defer_reseed(2_000 + RESEED_MS);
    assert!(!rng.needs_reseed(2_000 + RESEED_MS));
}

#[test]
fn fresh_bytes_change_what_follows() {
    let after = |fresh: Option<[u8; SEED_BYTES]>| {
        let mut rng = Csprng::new([4; SEED_BYTES], 0);
        if let Some(fresh) = fresh {
            rng.reseed(fresh, 0);
        }
        let mut out = [0u8; 32];
        assert!(rng.fill(&mut out));
        out
    };
    assert_ne!(after(None), after(Some([5; SEED_BYTES])));
    assert_ne!(after(Some([5; SEED_BYTES])), after(Some([6; SEED_BYTES])));
    // Even all-zero fresh bytes move the key on.
    assert_ne!(after(None), after(Some([0; SEED_BYTES])));
}
