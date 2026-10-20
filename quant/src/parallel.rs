//! Matrix-vector products over any range of rows, and on several workers.
//!
//! A [`Product`] is one weight matrix and the activation vectors it multiplies,
//! one per token, so batched prefill uses the same type as decoding. [`Product::run`]
//! computes any range of its rows. [`matvec_parallel`] splits the rows of one or
//! more products into chunks and hands them to the workers of a [`Parallel`]:
//! every row is computed by the same code whoever computes it, so the result is
//! the single-thread one bit for bit, for any number of workers.

use crate::q8k::Q8KVec;
use crate::{Tier, matvec, q4_k, q4_k_matvec, q6_k, q6_k_matvec, q8_0::QVec, row_bytes};
use core::ops::Range;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Workers that run one job at once, such as the llm service's thread pool. The
/// trait is safe to implement: a safe `broadcast` cannot keep `job` past its
/// return, so the rows a job writes are never written after
/// [`matvec_parallel`] returns. An implementation with unsafe code inside must
/// keep that rule.
pub trait Parallel {
    /// How many workers `broadcast` runs `job` on.
    fn workers(&self) -> usize;

    /// Calls `job(worker, workers)` on each worker, at once where it can, and
    /// returns when every call has returned. [`matvec_parallel`] stays correct
    /// if an implementation calls `job` fewer or more times than that.
    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync));
}

/// One worker: the caller.
pub struct Serial;

impl Parallel for Serial {
    fn workers(&self) -> usize {
        1
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        job(0, 1);
    }
}

#[derive(Clone, Copy)]
enum Weights<'a> {
    Q8_0(&'a [QVec]),
    Q4K(&'a [Q8KVec]),
    Q6K(&'a [Q8KVec]),
}

/// A weight matrix of `rows` rows in the model file's blocks, and the quantised
/// activation vectors (one per token) it multiplies. The constructors check every
/// size, so [`Product::run`] never reads past the weights.
#[derive(Clone, Copy)]
pub struct Product<'a> {
    w: &'a [u8],
    x: Weights<'a>,
    rows: usize,
    row_bytes: usize,
    tokens: usize,
}

impl<'a> Product<'a> {
    /// Q8_0 weights `w`, `rows` rows of `x[t].len()` values.
    pub fn q8_0(w: &'a [u8], rows: usize, x: &'a [QVec]) -> Self {
        let cols = same_len(x.iter().map(QVec::len));
        Self::new(w, rows, row_bytes(cols), Weights::Q8_0(x), x.len())
    }

    /// Q4_K weights `w`, `rows` rows of `x[t].len()` values.
    pub fn q4_k(w: &'a [u8], rows: usize, x: &'a [Q8KVec]) -> Self {
        let cols = same_len(x.iter().map(Q8KVec::len));
        Self::new(w, rows, q4_k::row_bytes(cols), Weights::Q4K(x), x.len())
    }

    /// Q6_K weights `w`, `rows` rows of `x[t].len()` values.
    pub fn q6_k(w: &'a [u8], rows: usize, x: &'a [Q8KVec]) -> Self {
        let cols = same_len(x.iter().map(Q8KVec::len));
        Self::new(w, rows, q6_k::row_bytes(cols), Weights::Q6K(x), x.len())
    }

    fn new(w: &'a [u8], rows: usize, row_bytes: usize, x: Weights<'a>, tokens: usize) -> Self {
        assert_eq!(
            Some(w.len()),
            rows.checked_mul(row_bytes),
            "{rows} rows of {row_bytes} bytes"
        );
        Self {
            w,
            x,
            rows,
            row_bytes,
            tokens,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// How many activation vectors (tokens) each row multiplies.
    pub fn tokens(&self) -> usize {
        self.tokens
    }

    /// Rows `rows` times every token's vector: `y[(r - rows.start) * tokens + t]`
    /// is row r times vector t. A row that comes to NaN gives the canonical NaN.
    pub fn run(&self, tier: Tier, rows: Range<usize>, y: &mut [f32]) {
        assert!(
            rows.start <= rows.end && rows.end <= self.rows,
            "rows {rows:?}"
        );
        assert_eq!(
            Some(y.len()),
            rows.len().checked_mul(self.tokens),
            "output size"
        );
        let w = &self.w[rows.start * self.row_bytes..rows.end * self.row_bytes];
        if self.tokens == 1 {
            return self.one(tier, w, 0, y);
        }
        let per_row = y.chunks_exact_mut(self.tokens);
        for (w, y) in w.chunks_exact(self.row_bytes.max(1)).zip(per_row) {
            for (t, y) in y.iter_mut().enumerate() {
                self.one(tier, w, t, core::slice::from_mut(y));
            }
        }
        if self.row_bytes == 0 {
            y.fill(0.0);
        }
    }

    /// The rows of `w` times vector `t`, one result each.
    fn one(&self, tier: Tier, w: &[u8], t: usize, y: &mut [f32]) {
        match self.x {
            Weights::Q8_0(x) => matvec(tier, w, &x[t], y),
            Weights::Q4K(x) => q4_k_matvec(tier, w, &x[t], y),
            Weights::Q6K(x) => q6_k_matvec(tier, w, &x[t], y),
        }
    }
}

/// The one length every vector has; at least one vector.
fn same_len(mut lens: impl Iterator<Item = usize>) -> usize {
    let first = lens.next().expect("a product needs at least one vector");
    assert!(lens.all(|len| len == first), "vectors of different lengths");
    first
}

/// The most products one [`matvec_parallel`] call takes: q, k and v share an
/// input, as do gate and up.
pub const MAX_PRODUCTS: usize = 8;
/// Rows per chunk are a multiple of this, so with one token a chunk's results
/// fill whole 64-byte cache lines and no two workers write the same line.
const CHUNK_ROWS: usize = 16;
/// Chunks per worker: a worker that is preempted or slow leaves its later chunks
/// to the others.
const CHUNKS_PER_WORKER: usize = 4;
/// `workers()` is taken as at most this, whatever a pool says.
const MAX_WORKERS: usize = 256;

/// A counter alone on its cache line.
#[repr(align(64))]
struct Line(AtomicUsize);

/// The outputs, written through raw pointers by whichever worker takes a chunk.
struct Outputs {
    y: [(*mut f32, usize); MAX_PRODUCTS],
}

// SAFETY: each pointer comes from a `&mut [f32]` that `matvec_parallel` holds for
// its whole call. Chunk indices come from one counter that only ever goes up to
// the number of chunks, so each chunk is taken once; chunks are disjoint row ranges checked to lie inside their output; and no
// one reads an output until every chunk is done and the call returns.
unsafe impl Sync for Outputs {}

/// Computes every row of every product in `items` into its output, spreading
/// chunks of rows over `par`'s workers. Products that share an input go in one
/// call, so a layer costs one broadcast per input rather than per matrix. Each
/// output holds `rows * tokens` values, as [`Product::run`] lays them out. The
/// result is the same bits as running each product alone on one thread.
///
/// Returns how many chunks the pool left for the caller to run itself: 0 when
/// `broadcast` ran the job on at least one worker and waited for it, since each
/// call takes chunks until none are left.
pub fn matvec_parallel(
    par: &dyn Parallel,
    tier: Tier,
    items: &mut [(Product<'_>, &mut [f32])],
) -> usize {
    assert!(
        items.len() <= MAX_PRODUCTS,
        "at most {MAX_PRODUCTS} products"
    );
    let workers = par.workers().clamp(1, MAX_WORKERS);
    let total_rows: usize = items.iter().map(|(p, _)| p.rows).sum();
    let chunk_rows = chunk_rows(total_rows, workers);
    let mut products: [Option<Product<'_>>; MAX_PRODUCTS] = [None; MAX_PRODUCTS];
    let mut outputs = Outputs {
        y: [(core::ptr::null_mut(), 0); MAX_PRODUCTS],
    };
    let mut chunks = 0;
    for (i, (product, y)) in items.iter_mut().enumerate() {
        assert_eq!(
            Some(y.len()),
            product.rows.checked_mul(product.tokens),
            "output size"
        );
        products[i] = Some(*product);
        outputs.y[i] = (y.as_mut_ptr(), y.len());
        chunks += product.rows.div_ceil(chunk_rows);
    }
    let (next, done) = (Line(AtomicUsize::new(0)), Line(AtomicUsize::new(0)));
    // Borrowed whole, so the job shares `Outputs` (Sync) and not its pointers.
    let outputs = &outputs;
    let job = |_: usize, _: usize| {
        loop {
            // Never counted past `chunks`, so however often a pool calls the
            // job, the counter cannot wrap and hand a chunk out twice.
            let mut seen = next.0.load(Ordering::Relaxed);
            let chunk = loop {
                if seen >= chunks {
                    return;
                }
                let swap = Ordering::Relaxed;
                match next.0.compare_exchange_weak(seen, seen + 1, swap, swap) {
                    Ok(_) => break seen,
                    Err(now) => seen = now,
                }
            };
            let (i, rows) = locate(&products, chunk, chunk_rows);
            let product = products[i].as_ref().expect("a located product");
            let (start, len) = (rows.start * product.tokens, rows.len() * product.tokens);
            let (base, out_len) = outputs.y[i];
            assert!(start + len <= out_len, "chunk inside its output");
            // SAFETY: `chunk` came from the counter above, so no other call
            // holds it; its rows are disjoint from every other chunk's and lie
            // inside output i (asserted); see `Outputs`.
            let y = unsafe { core::slice::from_raw_parts_mut(base.add(start), len) };
            product.run(tier, rows, y);
            done.0.fetch_add(1, Ordering::Release);
        }
    };
    par.broadcast(&job);
    // Whatever the pool left undone, the caller does.
    let left = chunks - done.0.load(Ordering::Acquire).min(chunks);
    job(0, 1);
    assert_eq!(
        done.0.load(Ordering::Acquire),
        chunks,
        "a chunk was taken but not finished"
    );
    left
}

/// Rows per chunk: about [`CHUNKS_PER_WORKER`] chunks per worker over all the
/// rows, a whole number of [`CHUNK_ROWS`], at least one of them.
pub(crate) fn chunk_rows(total_rows: usize, workers: usize) -> usize {
    let per_chunk = total_rows.div_ceil(workers * CHUNKS_PER_WORKER);
    per_chunk.next_multiple_of(CHUNK_ROWS).max(CHUNK_ROWS)
}

/// The product and the rows of chunk `chunk`: each product's rows are cut into
/// `chunk_rows`-row chunks in order, its last chunk possibly shorter.
pub(crate) fn locate(
    products: &[Option<Product<'_>>],
    mut chunk: usize,
    chunk_rows: usize,
) -> (usize, Range<usize>) {
    for (i, product) in products.iter().enumerate() {
        let Some(product) = product else { break };
        let here = product.rows.div_ceil(chunk_rows);
        if chunk < here {
            let start = chunk * chunk_rows;
            return (i, start..(start + chunk_rows).min(product.rows));
        }
        chunk -= here;
    }
    panic!("chunk past the products");
}
