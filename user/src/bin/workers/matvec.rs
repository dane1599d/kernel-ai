//! `mode=matvec`: an f32 matrix-vector product split by rows over a pool with one
//! worker on every CPU must give the same bits as one thread computing it alone,
//! round after round with new numbers.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use kabi::thread::MAX_WORKERS;
use user::pool::{Pool, share};
use user::{println, sys};

const ROWS: usize = 257;
const COLS: usize = 384;
const ROUNDS: u32 = 50;

/// A deterministic stream of floats in [-1, 1).
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// Row `row` of the matrix times the vector, summed in column order: the same
/// function, and so the same bits, whichever thread runs it.
#[inline(never)]
fn dot(matrix: &[f32], vector: &[f32], row: usize) -> f32 {
    let mut sum = 0.0f32;
    for (a, b) in matrix[row * COLS..(row + 1) * COLS].iter().zip(vector) {
        sum += a * b;
    }
    sum
}

pub fn matvec() -> i32 {
    let cpus = sys::metrics().map_or(1, |m| m.cpus as usize);
    let pool = match Pool::new(MAX_WORKERS) {
        Ok(pool) => pool,
        Err(error) => {
            println!("workers: no pool: {error:?}");
            return 130;
        }
    };
    if pool.workers() != cpus.min(MAX_WORKERS) {
        println!("workers: a pool of {} on {cpus} CPUs", pool.workers());
        return 131;
    }
    let rows_done: [AtomicUsize; MAX_WORKERS] = [const { AtomicUsize::new(0) }; MAX_WORKERS];
    let mut numbers = Numbers(0x5EED);
    let mut matrix = Vec::with_capacity(ROWS * COLS);
    let mut vector = Vec::with_capacity(COLS);
    let out: Vec<AtomicU32> = (0..ROWS).map(|_| AtomicU32::new(0)).collect();
    for round in 0..ROUNDS {
        matrix.clear();
        vector.clear();
        matrix.extend((0..ROWS * COLS).map(|_| numbers.next()));
        vector.extend((0..COLS).map(|_| numbers.next()));
        let alone: Vec<u32> = (0..ROWS)
            .map(|row| dot(&matrix, &vector, row).to_bits())
            .collect();
        let (matrix, vector, out, rows_done) = (&matrix, &vector, &out, &rows_done);
        pool.broadcast(&|worker, workers| {
            for row in share(ROWS, worker, workers) {
                out[row].store(dot(matrix, vector, row).to_bits(), Ordering::Relaxed);
                rows_done[worker].fetch_add(1, Ordering::Relaxed);
            }
        });
        if let Some(row) = (0..ROWS).find(|&row| out[row].load(Ordering::Relaxed) != alone[row]) {
            println!("workers: round {round}: row {row} differs from one thread's result");
            return 132;
        }
    }
    let per_worker: Vec<usize> = rows_done[..pool.workers()]
        .iter()
        .map(|done| done.load(Ordering::Relaxed))
        .collect();
    if per_worker.iter().sum::<usize>() != ROWS * ROUNDS as usize || per_worker.contains(&0) {
        println!("workers: rows per worker {per_worker:?}");
        return 133;
    }
    println!(
        "workers: matvec {ROWS}x{COLS} matched one thread bit for bit for {ROUNDS} rounds on {} workers, one per CPU; rows per worker {per_worker:?}",
        pool.workers()
    );
    0
}
