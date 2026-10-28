//! System-call numbers.
//!
//! Each lane has its own block of numbers, so two PRs adding calls never touch
//! the same lines: a lane gives its next call the next free number in its own
//! block and adds it in its own section below, after a blank line from the
//! section before. [`MAX`] is the end of the reserved blocks, so a new call never
//! changes it; an unused number in a block answers NotFound.
//!
//! | Numbers | Lane | Holds |
//! |---|---|---|
//! | 0-63 | P0 and P1, numbered as they landed | |
//! | 64-65 | model lane (S10-3) | MODEL_KV_LOAD, MODEL_KV_SAVE (interim) |
//! | 66-70 | S-io (P2) | PART_INFO, PART_READ, PART_WRITE, PART_FLUSH; ENDPOINT_MINT |
//! | 71-72 | K | PROC_STOP_ONCE, RULE_CHECK |
//! | 73-79 | G-core (P3) | DISPLAY_CLAIM, DISPLAY_FLUSH, DISPLAY_CURSOR, DISPLAY_CURSOR_MOVE, INPUT_READ, TERM_READ, INPUT_TO_CONSOLE |
//! | 80-89 | K, spare | MODEL_FILL 80 and 81 held (S-io, P2 task 10); ONCE_CHECK_FOR 82; P2 task 12's grants |
//! | 90-99 | S-engine (P2) | |
//! | 100-109 | P, packages (P2) | |
//! | 110-119 | A-core | |
//! | 120-129 | model lane | |
//! | 130-139 | D, drivers (ADR 0007) | RING_ATTACH, RING_NOTIFY, NOTIFY_WAIT, INPUT_PUSH |
//! | 140-149 | C, components | |
//! | 150-159 | V, virtual machines (P6) | |
//! | 160-169 | M, memory | MEM_RESERVE, MEM_COMMIT, MEM_RELEASE |
//! | 170-179 | U, updates (P4) | SLOT_INFO, SLOT_READ, SLOT_WRITE, SLOT_FLUSH, BOOTREC_GET, BOOTREC_SET, TRUST_ROOT, TRUST_TIME |
//! | 180-189 | G-core (P3), second block | ASK_READ, ASK_ANSWER, EVENT_POST, EVENT_READ, APP_CTL |
//! | 190-191 | not given yet | a lane asks lane K for a block |

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
/// exactly `name` (ExitReason Killed) and returns how many went. Needs
/// [`super::cap::SUPERVISE`]; the shell stops through [`PROC_STOP_ONCE`]. Permission for the protected names
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
// ---- Model lane, 64-65 (and 120-129) ----

/// **Interim** (S10-3; removed at P2 task 10, when the KV files move to
/// `/system/models/cache/`). `model_kv_load(region, offset, buf, len) -> len`:
/// reads `len` bytes at byte `offset` of KV region `region` of the `kai-cache`
/// partition (see [`crate::model::KV_REGIONS`]) into `buf`. `offset` and `len`
/// are whole sectors, `len` at most [`crate::model::KV_IO_BYTES`]. The model's
/// holder only (`cap::MODEL`); NotFound without a `kai-cache` partition that
/// holds every region.
pub const MODEL_KV_LOAD: u64 = 64;
/// **Interim**, as [`MODEL_KV_LOAD`]: `model_kv_save(region, offset, buf, len,
/// flush) -> len` writes `len` bytes of `buf` there; `flush` is 0, or 1 to make
/// every write so far durable after it, and anything else is Invalid.
/// Permission when the disk cannot be written.
pub const MODEL_KV_SAVE: u64 = 65;

// ---- S-io, 66-70 ----

/// `part_info(handle, &mut part::PartInfo) -> 0`: the partition a `Partition`
/// handle names: its length in sectors, the most one call moves, and whether it
/// takes writes. Needs INSPECT. NotFound when the disk has no such partition.
pub const PART_INFO: u64 = 66;
/// `part_read(handle, sector, buf, len) -> len`: reads `len` bytes from sector
/// `sector` of the partition into `buf`. `len` is whole sectors, 1 to
/// [`super::part::MAX_IO`] bytes; a range that is not wholly inside the
/// partition is Invalid, and nothing outside it can be named. Needs READ.
/// Unavailable when the disk fails the read.
pub const PART_READ: u64 = 67;
/// `part_write(handle, sector, buf, len) -> len`: as [`PART_READ`], writing `len`
/// bytes of `buf`. Durable only after [`PART_FLUSH`]. Needs WRITE; Permission on
/// a disk that cannot make writes durable.
pub const PART_WRITE: u64 = 68;
/// `part_flush(handle) -> 0`: makes every write before it durable. Needs WRITE.
pub const PART_FLUSH: u64 = 69;
/// `endpoint_mint(endpoint, kind, rights, badge) -> handle`: a new handle of a
/// server-backed kind (`Dir` or `File`) on the caller's own endpoint, which
/// needs MINT. Its rights are `rights` within that kind's
/// ([`super::fs`]); a call on it reaches this endpoint with `badge` and those
/// rights in [`super::handle::RecvInfo`]. The server hands it on with
/// REPLY_H.
pub const ENDPOINT_MINT: u64 = 70;

// ---- Lane K, 71-72 and 80-89 ----

/// `proc_stop_once(name_ptr, name_len, once_badge, entry_id) -> count`:
/// [`PROC_STOP`] for the shell, only with the user's yes (ADR 0001 D6). Needs a
/// Job handle with STOP ([`super::cap::STOP`]), and the caller must hold the
/// `ONCE` handle with badge `once_badge`, or, with `once_badge` 0, a handle to a
/// standing rule that covers the call ([`RULE_CHECK`]; Permission if none does). The kernel rebuilds the call shown on
/// menu `entry_id`, `process_stop("<name>")`, and checks it as [`ONCE_CHECK`]
/// does, which uses the `ONCE` up; only then does it stop, by PROC_STOP's rules.
/// Invalid for another call, NotFound for a badge the caller does not hold or the
/// kernel does not know, Revoked once its request ended.
pub const PROC_STOP_ONCE: u64 = 71;
/// `rule_check(pid, call_ptr, call_len) -> 0`: whether the process `pid` holds a
/// live handle to a standing rule (an "Always allow") that covers the call, in
/// canonical text (`service_stop("indexer")`), for a server that runs calls for
/// it (ADR 0002 D2a). Needs [`super::cap::SUPERVISE`]. Permission when no rule
/// covers it; NotFound for no such process.
pub const RULE_CHECK: u64 = 72;
/// `once_check_for(pid, badge, digest_ptr) -> 0`: [`ONCE_CHECK`] for a server
/// that runs a call for process `pid`: the `ONCE` must also be one `pid`
/// holds, so a process that only saw the badge cannot spend it. Needs
/// [`super::cap::SUPERVISE`]. NotFound when `pid` does not hold it; pid 0, the
/// kernel's own request, holds no table and is not checked for one. Numbered at
/// merge (lane K's spare block).
pub const ONCE_CHECK_FOR: u64 = 82;

// ---- G-core, 73-79 ----

/// `display_claim(&mut DisplayInfo) -> handle`: stops the console painting and
/// hands over the screen's back buffer as a kernel-owned `Shm` (MAP, READ,
/// WRITE; not DUPLICATE or TRANSFER), 4 KiB pages of XRGB8888 rows,
/// [`crate::display::DisplayInfo`] saying their shape. Needs a `Display` handle.
/// Busy while the screen is the console's after Ctrl+Alt+F12, until the second
/// press; NotFound without a display. The kernel revokes the `Shm` when its
/// holder exits or Ctrl+Alt+F12 is pressed (ADR 0005 §2).
pub const DISPLAY_CLAIM: u64 = 73;
/// `display_flush(rects, n) -> 0`: puts up to [`crate::display::MAX_FLUSH`]
/// rectangles ([`crate::display::FlushRect`]) of the claimed back buffer on
/// screen: a transfer and flush each on virtio-gpu, nothing on GOP. Needs the
/// claim.
pub const DISPLAY_FLUSH: u64 = 74;
/// `display_cursor(image, hot_x, hot_y) -> 0`: the 64x64 `0xAARRGGBB` hardware
/// cursor at `image` (16 KiB), its hot spot. NotFound without virtio-gpu (GOP
/// has none: `desktop` draws its own). Needs the claim.
pub const DISPLAY_CURSOR: u64 = 75;
/// `display_cursor_move(x, y) -> 0`: moves the hardware cursor's hot spot.
/// Needs the claim.
pub const DISPLAY_CURSOR_MOVE: u64 = 76;
/// `input_read(buf, cap, timeout_ms) -> n`: up to `cap` 16-byte
/// [`crate::input::InputEvent`]s from the kernel's one input queue, waiting up
/// to `timeout_ms` for the first. Needs an `Input` handle. While the console
/// holds the screen the queue gets nothing but a
/// [`crate::input::Kind::Screen`] event when it changes hands.
pub const INPUT_READ: u64 = 77;
/// `term_read(buf, cap, since) -> len`: the kernel console's rows changed after
/// sequence `since` (0 for all), with their kinds, the cursor and the rows
/// scrolled, in `termcore::mirror`'s format, for the terminal window (P3 task
/// 8). Read-only. Needs a `Term` handle, which the kernel gives only to the
/// service `term` running the program `term`. TooBig, with nothing written,
/// when `cap` is under what the rows take; `termcore::mirror::MAX_BYTES` always
/// fits.
pub const TERM_READ: u64 = 78;
/// `input_to_console(on) -> 0`: with `on` 1, keys go to the kernel console's
/// terminal instead of the input queue for about a second (a lease, renewed by
/// calling again), while the caller keeps showing on the screen; 0 ends it. Alt
/// and Alt+Tab still reach the queue. The kernel ends the lease itself on any
/// pointer button, Alt+Tab, Ctrl+Alt+F12, a claim, the holder's exit, and an
/// ask, which it answers on the console. Needs an `Input` handle and the claim.
pub const INPUT_TO_CONSOLE: u64 = 79;

// ---- S-engine, 90-99 ----

// ---- P, 100-109 ----

// ---- A-core, 110-119 ----

// ---- D, 130-139 ----

// ---- C, 140-149 ----

// ---- V, 150-159 ----

// ---- M, 160-169 ----

/// `mem_reserve(bytes) -> addr`: `bytes` (whole pages) of fresh address space,
/// with nothing mapped; it can be far larger than what will be committed (room
/// for guard regions). At most 64 reservations at once. Invalid for zero bytes,
/// NoMemory when the region or the table is full.
pub const MEM_RESERVE: u64 = 160;
/// `mem_commit(addr, bytes) -> 0`: maps zeroed read-write pages over
/// `[addr, addr + bytes)` of a reservation; pages committed already stay as they
/// are. Commits grow from the reservation's start: one past its committed end
/// is Invalid, as is one past its end. Counted against the page quota: past it,
/// NoMemory, with what fitted committed.
pub const MEM_COMMIT: u64 = 161;
/// `mem_release(addr) -> pages`: unmaps the reservation starting at `addr` and
/// gives every committed page back to the system; how many. NotFound for any
/// other address.
pub const MEM_RELEASE: u64 = 162;

// ---- U, 170-179 ----

/// `slot_info(handle, slot, &mut update::SlotInfo) -> 0`: slot `slot`'s
/// partition ([`super::update::R`], `A` or `B`) on the boot disk, the slot this
/// boot runs in, and whether SLOT_WRITE would take it now. Needs an `Update`
/// handle with INSPECT. NotFound when the disk has no such partition.
pub const SLOT_INFO: u64 = 170;
/// `slot_read(handle, slot, sector, buf, len) -> len`: reads `len` bytes from
/// sector `sector` of the slot. `len` is whole sectors, 1 to
/// [`super::update::MAX_IO`] bytes, wholly inside the slot; Invalid otherwise.
/// Needs READ.
pub const SLOT_READ: u64 = 171;
/// `slot_write(handle, slot, sector, buf, len) -> len`: as [`SLOT_READ`],
/// writing. Permission for the slot this boot runs in, and for R unless that
/// slot is committed (ADR 0008 §10). Durable only after [`SLOT_FLUSH`]. Needs
/// WRITE.
pub const SLOT_WRITE: u64 = 172;
/// `slot_flush(handle, slot) -> 0`: makes every write to the slot's disk before
/// it durable. Needs WRITE.
pub const SLOT_FLUSH: u64 = 173;
/// `bootrec_get(handle, buf) -> 64`: `KaiBootState` as the firmware holds it,
/// 64 bytes into `buf`. NotFound when there is none, Unavailable without
/// runtime services. Needs READ. Like BOOTREC_SET and SLOT_WRITE, Busy until
/// this boot's own boot lever has written the record (`kernel/src/slots.rs`).
pub const BOOTREC_GET: u64 = 174;
/// `bootrec_set(handle, buf, one_shot, restart) -> 0`: writes the 64-byte
/// record at `buf` (its CRC must hold), and Limine's one-shot for `one_shot`
/// (a slot, or [`super::update::NONE`] to delete it); a one-shot naming a slot
/// the record says may not boot is Invalid. With `restart` 1 the machine then
/// resets into it. Needs WRITE.
pub const BOOTREC_SET: u64 = 175;
/// `trust_root(handle, buf, cap) -> len`: the TUF root this slot was built
/// with (ADR 0008 §2, `kai.root.json` in its bundle), the device's trust
/// anchor until `/system/trust` caches newer ones. NotFound without one;
/// Invalid if longer than `cap`. Needs READ.
pub const TRUST_ROOT: u64 = 176;
/// `trust_time(handle) -> seconds since 1970 UTC`: Limine's date at boot plus
/// the time since boot, the clock TUF's expiry checks use (ADR 0008 §7).
/// NotFound when the firmware gave no date. Needs READ.
pub const TRUST_TIME: u64 = 177;

// ---- G-core, second block, 180-189 ----

/// `ask_read(handle, buf, cap) -> len`: the ask waiting for `desktop`'s card, as
/// its log entry id, its call in canonical text (`termcore::once::call_text`)
/// and its question, one per line; 0 when none waits. Reading marks the trusted
/// layer alive: the kernel posts asks to the card only while `desktop` reads,
/// and asks on the console otherwise (ADR 0005 §7). TooBig, with nothing
/// written, when `cap` is too small. Needs an `Ask` handle, which the kernel
/// gives only to the service `desktop` running the program `desktop`.
pub const ASK_READ: u64 = 180;
/// `ask_answer(handle, id, yes, digest_ptr) -> 0`: answers ask `id` from the
/// card, Run (`yes` 1) or Don't (0). `digest_ptr` points at the 32-byte
/// `termcore::once::digest_call` of the call the card showed; NotFound, and
/// no answer, unless that is the posted ask's. The kernel then mints the
/// `ONCE` as for the console's menu. Needs an `Ask` handle.
pub const ASK_ANSWER: u64 = 181;
/// `event_post(line_ptr, line_len) -> 0`: puts one event for standing
/// instructions in the kernel's box (P3 task 11), one line as
/// [`super::event::Event::parse`] reads it: a window event or idle. Invalid for
/// any other line, and for `time`, which the kernel adds itself. Only the
/// display holder posts: it needs [`super::cap::DISPLAY`], which the kernel
/// gives only to `desktop`. The box keeps [`super::event::BOX`] events; past
/// that the oldest goes, and is counted.
pub const EVENT_POST: u64 = 182;
/// `event_read(buf, cap) -> len`: takes the box's events, one line each,
/// oldest first, then a `time <minute>` (UTC) for each minute of the day begun
/// since the last read; first `dropped <n>` when events were lost since the
/// last read. 0 when none wait. Only the process serving `shell` may read.
/// TooBig, with nothing taken, when `cap` is under
/// [`super::event::READ_BYTES`].
pub const EVENT_READ: u64 = 183;
/// `app_ctl(name_ptr, name_len, verb, buf, cap) -> len`: the kernel sends
/// `ctl suspend` (`verb` 0) or `ctl resume` (1) to the app service `name`, as
/// its own request, and writes the reply into `buf`. For the supervisor, which
/// runs `app_suspend` and `app_resume` for a standing instruction after
/// RULE_CHECK (P3 task 11): needs [`super::cap::SUPERVISE`]. NotFound with no
/// such service; TimedOut when it does not answer.
pub const APP_CTL: u64 = 184;

/// The end of the reserved blocks (see the table above): the highest number a
/// call may have. A call that is not there yet answers NotFound.
pub const MAX: u64 = 191;

#[cfg(test)]
mod tests {
    /// Every call constant in this file, by name and number, read from its own
    /// source so a new call needs no edit here.
    fn calls() -> alloc::vec::Vec<(&'static str, u64)> {
        include_str!("call.rs")
            .lines()
            .filter_map(|line| line.strip_prefix("pub const "))
            .filter_map(|rest| {
                let (name, value) = rest.split_once(": u64 = ")?;
                let value = value.strip_suffix(';')?;
                // Flags such as WRITE_LOG_ONLY are not call numbers.
                if value.contains("<<") || name == "MAX" {
                    return None;
                }
                Some((name, value.parse().ok()?))
            })
            .collect()
    }

    #[test]
    fn every_call_has_its_own_number_within_max() {
        let calls = calls();
        assert!(calls.len() > 60, "{} calls read", calls.len());
        for (i, (name, n)) in calls.iter().enumerate() {
            assert!(*n <= super::MAX, "{name} = {n} is past MAX");
            for (other, m) in &calls[i + 1..] {
                assert_ne!(n, m, "{name} and {other} share {n}");
            }
        }
    }
}
