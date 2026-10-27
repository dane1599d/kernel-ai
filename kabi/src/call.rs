//! System-call numbers.

/// `exit(code: i32) -> !`
pub const EXIT: u64 = 0;
/// `write(ptr, len, flags) -> len`: text to the console, prefixed with the program
/// name. With [`WRITE_LOG_ONLY`] in `flags` it goes to the log (serial and the
/// terminal's `:log`) and not the screen.
pub const WRITE: u64 = 1;
/// [`WRITE`]'s flag for text that belongs in the log rather than on the screen: an
/// audit trail, a progress note nobody asked to see.
pub const WRITE_LOG_ONLY: u64 = 1 << 0;
/// `sleep_ms(ms)`
pub const SLEEP: u64 = 2;
/// `yield()`
pub const YIELD: u64 = 3;
/// `brk(grow_bytes) -> heap end`: grows the heap by whole pages; 0 just reports.
pub const BRK: u64 = 4;
/// `time_ns() -> nanoseconds since boot`
pub const TIME: u64 = 5;
/// `args(buf, cap) -> len`: the argument string the program was started with.
pub const ARGS: u64 = 6;
/// `spawn(&SpawnRequest) -> pid` (needs [`super::cap::SUPERVISE`])
pub const SPAWN: u64 = 7;
/// `wait_child(&mut ExitRecord, timeout_ms) -> 1 if a record was written, 0 on
/// timeout, 2 if a request waits for the caller's own service` (the supervisor
/// serves `supervisor` between exits; it takes the request with `svc_recv`)
pub const WAIT_CHILD: u64 = 8;
/// `kill(pid)`
pub const KILL: u64 = 9;
/// `state_save(ptr, len)`: replaces the kernel-held supervisor state.
pub const STATE_SAVE: u64 = 10;
/// `state_load(buf, cap) -> len`
pub const STATE_LOAD: u64 = 11;
/// `plan(buf, cap) -> len`: the services to run, one `name args...` per line.
pub const PLAN: u64 = 12;
/// `svc_recv(buf, cap, timeout_ms, &mut caller_pid) -> len`: the next request for
/// this service, and who sent it (0 = the kernel) (needs [`super::cap::SERVE`]).
pub const SVC_RECV: u64 = 13;
/// `svc_reply(ptr, len)`: answers the request taken by the last `svc_recv`.
pub const SVC_REPLY: u64 = 14;
/// `http(&HttpRequest) -> response len`: sends the request bytes to the LLM
/// server and returns what it sends back before closing the connection
/// (needs [`super::cap::NET`]).
pub const HTTP: u64 = 15;
/// `list(buf, cap) -> len`: running services, one `pid name` per line, so a
/// restarted supervisor can find the services it already started.
pub const LIST: u64 = 16;
/// Retired: calling a service by name. The number stays taken and answers
/// NotFound; [`CALL_H`] calls through a route handle instead (ADR 0001 D3).
pub const CALL: u64 = 17;
/// `metrics(&mut Metrics) -> 0`: scheduler, memory and process numbers.
pub const METRICS: u64 = 18;
/// `policy(id, preset)`: installs scheduling policy [`super::policy`] `id` on every
/// CPU, ending any trial (needs [`super::cap::POLICY`]); `preset` matters only for
/// `classes`. Busy within 100 ms of the last switch.
pub const POLICY: u64 = 19;
/// `interrupted() -> 1 or 0`: whether the user pressed Ctrl-C since the last
/// call; reading clears it.
pub const INTERRUPTED: u64 = 20;
/// `comp_fetch(label_ptr, label_len, buf, cap) -> len`: copies version `label` of
/// the caller's component into `buf` when it fits; the length either way.
/// Needs [`cap::COMPONENT`].
pub const COMP_FETCH: u64 = 21;
/// `comp_event(&live::Event) -> 0`: reports a ledger event; the kernel checks
/// it against the ledger rules and refuses what they do not allow.
pub const COMP_EVENT: u64 = 22;
/// `comp_restore(&mut live::Restore, buf, cap) -> len`: the ledger, and the
/// active version's checkpoint copied into `buf` when it fits.
pub const COMP_RESTORE: u64 = 23;
/// `comp_put(&live::Put) -> 0`: uploads a version of component `name` into the
/// store, under the label the store assigns next (`e1`, `e2`, ...), with the
/// label it was built from and the git commit holding its source. The kernel
/// checks the artifact against the upload rules (see [`crate::live::MAX_UPLOAD`]).
/// Needs [`super::cap::PUBLISH`].
pub const COMP_PUT: u64 = 24;
/// `comp_control(&live::Control) -> reply len`: sends `ctl <request>` to a live
/// component as the kernel (caller 0), waiting at most 60 s. Only `status`,
/// `source [file]` and `try <version>` are relayed (see
/// [`crate::live::relayable`]); anything else is Invalid, and a name that is
/// not a live component's host is NotFound. Needs [`super::cap::PUBLISH`].
pub const COMP_CONTROL: u64 = 25;
/// `comp_read(&live::Read) -> len`: a component's [`crate::live::Info`], or the bytes
/// of one of its versions copied into the buffer when they fit (the length
/// either way). A read is not a fetch: it gives no right to report events.
/// Needs [`super::cap::PUBLISH`].
pub const COMP_READ: u64 = 26;
/// `build(&HttpRequest) -> response len`: as [`HTTP`], to the build service named
/// by `build=` on the kernel command line, and only `POST /sync`; anything else
/// is Invalid (builds go through [`BUILD_START`]). Needs [`super::cap::PUBLISH`].
pub const BUILD: u64 = 27;
/// `model_map() -> len`: maps the model the kernel loaded from the `kai-model`
/// disk at [`super::layout::MODEL_BASE`], read-only and not executable, by
/// linking the kernel's window of 2 MiB pages, which takes nothing from the
/// page quota; returns its length in bytes, and calling it again maps nothing
/// new. NotFound when no model is loaded, Invalid if the window cannot be
/// linked. Needs [`super::cap::MODEL`].
pub const MODEL_MAP: u64 = 28;
/// `thread_spawn(entry, arg, cpu, stack_pages) -> index`: starts a worker of this
/// process at `entry` with `arg` in RDI, pinned to CPU `cpu` (below the CPUs
/// online), on a stack of `stack_pages` pages (see [`super::thread`]). Only the
/// main thread of a holder of [`super::cap::THREADS`] may. Busy when it already
/// runs [`super::thread::MAX_WORKERS`] workers (for a moment also after one
/// exits) or the kernel's threads run out; NoMemory past the page quota.
pub const THREAD_SPAWN: u64 = 29;
/// `thread_exit() -> !`: ends the calling worker. Invalid from the main thread,
/// which ends the whole process with [`EXIT`].
pub const THREAD_EXIT: u64 = 30;
/// `futex_wait(addr, expected, timeout_ms) -> 0 or 1`: sleeps while the aligned
/// 32-bit word at `addr` in this process holds `expected`, until a
/// [`FUTEX_WAKE`] for it (0), `timeout_ms` (0, or more than an hour, means
/// none; TimedOut) or a kill (Dead); 1 at once when the word holds something else.
pub const FUTEX_WAIT: u64 = 31;
/// `futex_wake(addr, count) -> woken`: wakes up to `count` threads of this
/// process waiting on the word at `addr`.
pub const FUTEX_WAKE: u64 = 32;
/// `sched_stats(&mut sched::SchedStats, window_ms) -> records`: watches the
/// scheduler for `window_ms` (1 to 10,000) and returns every thread's numbers over
/// that window; 0 returns at once with only the installed policy and the trial's
/// status. Needs [`super::cap::POLICY`]. See [`super::sched`].
pub const SCHED_STATS: u64 = 33;
/// `sched_trial(&sched::TrialRequest) -> trial id`: starts a kernel-owned trial of
/// a policy against the installed one. Busy while a trial runs; Invalid for a spec,
/// label, target or plan outside the bounds. Needs [`super::cap::POLICY`].
pub const SCHED_TRIAL: u64 = 34;
/// `build_start(&HttpRequest) -> 0`: sends a `POST /build` (only) to [`BUILD`]'s service
/// on a kernel thread, for at most 300 s. Busy until its reply is collected. Needs PUBLISH.
pub const BUILD_START: u64 = 35;
/// `build_poll(buf, cap) -> len`: copies the reply, ending the job, only if it fits (cap 0
/// looks). Busy while it runs, NotFound with none; an exchange error comes once. PUBLISH.
pub const BUILD_POLL: u64 = 36;
/// `resolve(host_ptr, host_len, out_ptr, out_cap) -> addresses`: the IPv4
/// addresses of `host`, a host name or a dotted address, from the kernel's DNS
/// resolver and its TTL cache. Up to `out_cap` of them are written to `out_ptr`,
/// four bytes each in network order, and the number the answer had (at most
/// [`super::net::MAX_ADDRESSES`]) is returned. Invalid for a malformed name or one
/// longer than [`super::net::MAX_HOST`], NotFound for a host with no IPv4 address,
/// Unavailable with no DNS server, TimedOut after
/// [`super::net::RESOLVE_TIMEOUT_MS`], Interrupted on Ctrl-C. Needs [`super::cap::RESOLVE`].
pub const RESOLVE: u64 = 37;
/// `abi_info(&mut handle::AbiInfo) -> 0`: the ABI version, the highest call
/// number and the handle table's size (ADR 0001 D10). Needs nothing.
pub const ABI_INFO: u64 = 38;
/// `startup(out: &mut [handle::HandleInfo], cap) -> count`: the handles the
/// process got at spawn from its capability bits, up to `cap` of them written to
/// `out`; still open ones only. Needs nothing.
pub const STARTUP: u64 = 39;
/// `handle_info(handle, &mut handle::HandleInfo) -> 0`: a handle's kind, rights
/// and flags. BadHandle for one that is not open.
pub const HANDLE_INFO: u64 = 40;
/// `handle_dup(handle, rights) -> handle`: a second handle to the same object
/// with `rights`, which must be a subset of the source's (Invalid otherwise).
/// Needs DUPLICATE on the source.
pub const HANDLE_DUP: u64 = 41;
/// `handle_replace(handle, rights) -> handle`: swaps a handle for one with fewer
/// `rights` (a subset; Invalid otherwise); the old one is closed either way.
pub const HANDLE_REPLACE: u64 = 42;
/// `handle_close(handle) -> 0`: closes a handle for good. The right it carried is
/// gone: a call that needs it fails with Permission from then on.
pub const HANDLE_CLOSE: u64 = 43;
/// `call_h(route, &handle::CallH) -> reply len`: sends a request through a
/// `Service` route with CALL and waits for the reply, moving up to
/// [`super::handle::MAX_TRANSFER`] handles each way (each needs TRANSFER, and
/// the receiver's capability bits must allow its kind; Permission otherwise,
/// and nothing moves). The handles the reply carries are written to
/// `handles_out`, the rest of it zero.
pub const CALL_H: u64 = 44;
/// `recv_h(buf, cap, timeout_ms, &mut handle::RecvInfo) -> request len`: as
/// [`SVC_RECV`], also reporting the caller's pid (0 for the kernel), the route's
/// badge and rights, and the handles the request carried, now in this process.
pub const RECV_H: u64 = 45;
/// `reply_h(ptr, len, handles_ptr, handles_len) -> len`: as [`SVC_REPLY`],
/// moving up to [`super::handle::MAX_TRANSFER`] handles to the caller.
pub const REPLY_H: u64 = 46;
/// `grant_new(parent, life, label_ptr, label_len) -> handle`: a new node of the
/// grant tree under the node the `Grant` handle `parent` names, which needs MINT;
/// `life` is 0 (request), 1 (session) or 2 (persistent), the label at most 24
/// bytes. Returns a `Grant` handle to the new node with MINT, REVOKE, INSPECT,
/// DUPLICATE and TRANSFER, which itself dies with the node (ADR 0001 D5).
pub const GRANT_NEW: u64 = 47;
/// `grant_revoke(grant) -> nodes`: revokes the node the `Grant` handle names
/// (needs REVOKE) and every node below it. Every handle given under them answers
/// Revoked from then on, and calls in flight through them fail with Revoked.
pub const GRANT_REVOKE: u64 = 48;
/// `grant_dup(handle, rights, grant) -> handle`: as [`HANDLE_DUP`], with the new
/// handle given under the node the `Grant` handle `grant` names (needs MINT), so
/// revoking that node revokes it.
pub const GRANT_DUP: u64 = 49;
/// `once_check(badge, digest_ptr) -> 0`: whether the 32-byte digest at
/// `digest_ptr` is the one the trusted path holds for the `ONCE` badge `badge`
/// (ADR 0001 D6). A match uses it up. Invalid for another digest, NotFound for
/// no digest under the badge, Revoked once its request ended.
pub const ONCE_CHECK: u64 = 50;
/// `dev_list(buf, cap) -> len`: the bound devices, one
/// [`crate::device::DeviceRecord`] of 64 bytes each, as many whole records as
/// fit in `cap` bytes; returns the bytes written. ADR 0007 section 3: no
/// handle needed yet, as for METRICS (ADR 0001's INSPECT check comes to both).
pub const DEV_LIST: u64 = 51;
/// `proc_stop(name_ptr, name_len) -> count`: stops every live process named
/// exactly `name` (ExitReason Killed) and returns how many went. Needs a Job
/// handle with STOP ([`super::cap::STOP`]). Permission for the protected names
/// (`shell`, `supervisor`, `llm`) and the caller's own; Invalid for a service the
/// supervisor runs (stop those through the supervisor's `stop <name>`); NotFound
/// when no process has the name.
pub const PROC_STOP: u64 = 52;
/// `shm_create(pages) -> handle`: new zeroed shared pages, counted against the
/// caller's page quota, and an `Shm` handle with MAP, READ, WRITE, DUPLICATE,
/// TRANSFER and INSPECT (ADR 0001 D4). TooBig past [`crate::layout::SHM_SLOT`];
/// Busy when every object is taken.
pub const SHM_CREATE: u64 = 53;
/// `shm_map(handle, write) -> address`: maps the object at its place in
/// [`crate::layout::SHM_BASE`], read-only (MAP and READ) or read-write (MAP, READ
/// and WRITE, `write` = 1), never executable. Busy if this process maps it already.
/// Once the grant node its handle came through is revoked, every page reads as
/// zero and writes reach no one.
pub const SHM_MAP: u64 = 54;
/// `shm_unmap(address) -> 0`: removes the mapping that starts at `address`. The
/// pages are freed when no mapping and no handle is left.
pub const SHM_UNMAP: u64 = 55;
/// `job_spawn(job, &JobSpawn) -> pid`: starts the program the Job names
/// (`scriptrun`) as the caller's child, with no capability bits and only the
/// handles it moves in (each with TRANSFER, of a kind the caller may hold). Needs
/// CREATE on the Job. Busy while one runs.
pub const JOB_SPAWN: u64 = 56;
/// `job_wait(job, &mut ExitRecord, timeout_ms) -> 1 if a record was written, 0 on
/// timeout`: the next exit of one of the caller's job children. Needs WAIT.
pub const JOB_WAIT: u64 = 57;
/// `job_pool(job, n) -> count`: keeps up to `n` (at most 4) idle `scriptrun`
/// children for the caller, with no handles (ADR 0001's pooled `scriptrun`).
/// Needs CREATE on the Job.
pub const JOB_POOL: u64 = 58;
/// `job_claim(job, &JobSpawn) -> pid`: gives an idle pooled child the request's
/// arguments and handles (as JOB_SPAWN moves them) and wakes it; starts one when
/// none is idle. Needs CREATE. Its result comes back through JOB_WAIT.
pub const JOB_CLAIM: u64 = 59;
/// `job_take(buf, cap) -> len`, in a pooled child: waits for a claim and copies
/// its arguments; the claim's handles are in STARTUP. TimedOut with no claim for
/// a while; Permission outside a job child.
pub const JOB_TAKE: u64 = 60;
/// `job_done(code) -> 0`, in a pooled child: reports `code` to the parent's
/// JOB_WAIT (reason Exited, detail `code`), closes every handle it holds and goes
/// back to idle. Invalid with no claim taken.
pub const JOB_DONE: u64 = 61;
/// `job_kill(job, pid) -> 0`: ends the caller's job child `pid` (a time limit,
/// Ctrl-C); a pooled child dies too and never goes back to idle. JOB_WAIT then
/// reports it Killed. NotFound for a pid that is not the caller's job child.
pub const JOB_KILL: u64 = 62;
/// `thread_pin(cpu) -> 0`: pins the calling thread (a process's main thread or a
/// worker) to `cpu` and moves it there; load balancing leaves it alone until
/// `cpu` = `u64::MAX` unpins it. Invalid for a CPU at or past the online count.
/// Needs [`super::cap::THREADS`]. (llm keeps its main thread beside its workers'
/// CPUs.)
pub const THREAD_PIN: u64 = 63;
/// The highest call number.
pub const MAX: u64 = THREAD_PIN;

/// **Spike S05-1 only** (ADR 0005; the kernel's `spikes` feature; never merged).
/// `spike_display_map(&mut [u64; 3]) -> address`: the console stops drawing and
/// the display's back buffer is mapped read-write at
/// [`crate::layout::SPIKE_DISPLAY_BASE`] in 4 KiB pages; writes width, height and
/// stride (pixels). NotFound without a display.
pub const SPIKE_DISPLAY_MAP: u64 = 0x5051;
/// **Spike S05-1 only.** `spike_display_flush(rects, n <= 32) -> 0`: each of `n`
/// `[x, y, width, height]` u32 rectangles of the back buffer goes on screen.
pub const SPIKE_DISPLAY_FLUSH: u64 = 0x5052;
/// **Spike S05-1 only.** `spike_share_map(id < 4, bytes) -> address`: shared
/// zeroed pages, the same for every process that maps `id`, read-write at
/// `SPIKE_SHARE_BASE + id * SPIKE_SHARE_SLOT`. The first call sets the size.
pub const SPIKE_SHARE_MAP: u64 = 0x5053;
/// **Spike S05-1 only.** As FUTEX_WAIT, but keyed by the word's physical frame,
/// so a waiter in one process is woken from another through a shared page.
pub const SPIKE_BELL_WAIT: u64 = 0x5054;
/// **Spike S05-1 only.** As FUTEX_WAKE, for [`SPIKE_BELL_WAIT`].
pub const SPIKE_BELL_RING: u64 = 0x5055;
/// **Spike S05-1 only.** `spike_mark() -> 0`: the kernel logs "ready for keys",
/// so xtask takes the next screendump.
pub const SPIKE_MARK: u64 = 0x5056;
