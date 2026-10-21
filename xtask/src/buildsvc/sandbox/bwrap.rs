//! The bubblewrap command a sandboxed build runs, identical on CI (spawned
//! directly) and on the Windows dev host (spawned through wsl.exe into the
//! `kai-build` distro), and the two root-owned policy files it relies on: the
//! `kai-bwrap` launcher and the seccomp filter.

/// The launcher and the filter, installed root-owned, outside every path a build
/// can write, so one build cannot weaken the policy for the next.
pub const LAUNCHER: &str = "/usr/local/bin/kai-bwrap";
pub const FILTER: &str = "/usr/local/lib/kai/sandbox.bpf";

/// Bounds on one sandboxed build.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Address space of each process, in bytes.
    pub address_space: u64,
    /// Processes the build user may have at once.
    pub processes: u64,
    /// Largest file a process may write.
    pub file_size: u64,
    /// Size of the build's `/tmp`.
    pub tmp_bytes: u64,
}

/// Starting values, to be tuned from measured builds; generous enough for rustc
/// and rust-lld with LTO, small enough that a runaway build fails on its own.
pub const LIMITS: Limits = Limits {
    address_space: 8 << 30,
    processes: 512,
    file_size: 256 << 20,
    tmp_bytes: 256 << 20,
};

/// Where a build's pieces live on the Linux side. The sealed build sees them at
/// the same absolute paths as the warm-up outside it, so cargo's fingerprints
/// line up and only the edited crates recompile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// The job's source tree, read-only inside.
    pub src: String,
    /// The target directory, the one place the build writes.
    pub edit_build: String,
    pub cargo_home: String,
    pub rustup_home: String,
}

/// The `kai-bwrap` launcher. Its first argument is the time limit in seconds;
/// the rest go to bwrap. It opens the filter on the Linux side, because a file
/// descriptor cannot cross from Windows into WSL.
pub fn launcher(limits: &Limits) -> String {
    format!(
        "#!/bin/sh\n\
         # kai-bwrap: the Kernel AI build sandbox launcher, written by\n\
         # `cargo xtask sandbox-files`. Install it root-owned at {LAUNCHER}.\n\
         set -eu\n\
         limit=\"$1\"\n\
         shift\n\
         case \"$limit\" in ''|*[!0-9]*) echo \"kai-bwrap: bad time limit\" >&2; exit 2 ;; esac\n\
         exec 9<\"{FILTER}\"\n\
         exec /usr/bin/prlimit --as={} --nproc={} --fsize={} \\\n\
         \x20 /usr/bin/timeout -s KILL \"$limit\" /usr/bin/bwrap --seccomp 9 \"$@\"\n",
        limits.address_space, limits.processes, limits.file_size
    )
}

/// The cargo command a build runs: the boot versions' target and profile.
pub fn cargo_args(krate: &str) -> Vec<String> {
    [
        "build",
        "--frozen",
        "-p",
        krate,
        "--target",
        crate::components::TARGET,
        "--profile",
        "component",
        "--message-format",
        "short",
        "--color",
        "never",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The sealed build's argv, starting with the launcher. `program` is what runs
/// inside: cargo with `args`, or `/usr/bin/true` for the start-up probe.
pub fn command(
    paths: &Paths,
    label: &str,
    seconds: u64,
    tmp_bytes: u64,
    program: &str,
    args: &[String],
) -> Vec<String> {
    let path = format!("{}/bin:/usr/bin:/bin", paths.cargo_home);
    let mut argv: Vec<String> = vec![LAUNCHER.into(), seconds.to_string()];
    let mut add = |items: &[&str]| argv.extend(items.iter().map(|s| s.to_string()));
    // Every namespace, no new user namespaces inside, no controlling terminal,
    // and nothing left running when the launcher goes.
    add(&[
        "--unshare-user",
        "--unshare-all",
        "--disable-userns",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
    ]);
    add(&["--setenv", "PATH", &path]);
    add(&["--setenv", "HOME", "/tmp"]);
    add(&["--setenv", "CARGO_HOME", &paths.cargo_home]);
    add(&["--setenv", "RUSTUP_HOME", &paths.rustup_home]);
    add(&["--setenv", "RUSTUP_AUTO_INSTALL", "0"]);
    add(&["--setenv", "CARGO_TARGET_DIR", &paths.edit_build]);
    add(&["--setenv", "KAI_VERSION", label]);
    // The system and the toolchain, read-only; Ubuntu merges /bin and /lib into /usr.
    add(&["--ro-bind", "/usr", "/usr"]);
    add(&["--symlink", "usr/bin", "/bin"]);
    add(&["--symlink", "usr/lib", "/lib"]);
    add(&["--symlink", "usr/lib64", "/lib64"]);
    add(&["--ro-bind-try", "/etc/alternatives", "/etc/alternatives"]);
    add(&["--ro-bind-try", "/etc/ld.so.cache", "/etc/ld.so.cache"]);
    add(&["--ro-bind", &paths.rustup_home, &paths.rustup_home]);
    add(&["--ro-bind", &paths.cargo_home, &paths.cargo_home]);
    // The job's source, read-only, and the one writable tree.
    add(&["--ro-bind", &paths.src, &paths.src]);
    add(&["--bind", &paths.edit_build, &paths.edit_build]);
    add(&["--proc", "/proc", "--dev", "/dev"]);
    // --size applies to the --tmpfs after it.
    add(&["--size", &tmp_bytes.to_string(), "--tmpfs", "/tmp"]);
    add(&["--chdir", &paths.src]);
    argv.push(program.to_string());
    argv.extend(args.iter().cloned());
    argv
}

/// One classic BPF instruction, laid out as the kernel's `struct sock_filter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insn {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

pub const LD_W_ABS: u16 = 0x20;
pub const JEQ_K: u16 = 0x15;
pub const JSET_K: u16 = 0x45;
pub const RET_K: u16 = 0x06;

pub const ARCH_X86_64: u32 = 0xC000_003E;
/// Set in the syscall number by the x32 ABI, which reports the x86-64 arch too.
pub const X32_BIT: u32 = 0x4000_0000;
pub const RET_ALLOW: u32 = 0x7fff_0000;
pub const RET_ERRNO: u32 = 0x0005_0000;
pub const RET_KILL_PROCESS: u32 = 0x8000_0000;
pub const EPERM: u32 = 1;
pub const EAFNOSUPPORT: u32 = 97;

const NR_SOCKET: u32 = 41;
const NR_SOCKETPAIR: u32 = 53;
const NR_IO_URING: [u32; 3] = [425, 426, 427];
/// Address families a build may open: local sockets, and IP, which the empty
/// network namespace leaves nowhere to go. AF_VSOCK, which reaches the Windows
/// host from WSL whatever the namespace, is not among them.
const FAMILIES: [u32; 3] = [1, 2, 10];

/// Offsets into `struct seccomp_data`.
const DATA_NR: u32 = 0;
const DATA_ARCH: u32 = 4;
const DATA_ARG0: u32 = 16;

/// The seccomp filter: only the x86-64 ABI; no x32 calls; sockets only of the
/// families above; no io_uring, which can make sockets without a socket call.
pub fn filter() -> Vec<Insn> {
    let ins = |code, k, jt, jf| Insn { code, jt, jf, k };
    let ld = |offset| ins(LD_W_ABS, offset, 0, 0);
    let ret = |k| ins(RET_K, k, 0, 0);
    let mut program = vec![
        ld(DATA_ARCH),
        ins(JEQ_K, ARCH_X86_64, 1, 0),
        ret(RET_KILL_PROCESS),
        ld(DATA_NR),
    ];
    // Jumps are filled in once the targets are known.
    let x32 = program.len();
    program.push(ins(JSET_K, X32_BIT, 0, 0));
    let sockets: Vec<usize> = [NR_SOCKET, NR_SOCKETPAIR]
        .iter()
        .map(|&nr| {
            program.push(ins(JEQ_K, nr, 0, 0));
            program.len() - 1
        })
        .collect();
    let urings: Vec<usize> = NR_IO_URING
        .iter()
        .map(|&nr| {
            program.push(ins(JEQ_K, nr, 0, 0));
            program.len() - 1
        })
        .collect();
    program.push(ret(RET_ALLOW));
    let socket_check = program.len();
    program.push(ld(DATA_ARG0));
    let families: Vec<usize> = FAMILIES
        .iter()
        .map(|&family| {
            program.push(ins(JEQ_K, family, 0, 0));
            program.len() - 1
        })
        .collect();
    program.push(ret(RET_ERRNO | EAFNOSUPPORT));
    let allow = program.len();
    program.push(ret(RET_ALLOW));
    let deny = program.len();
    program.push(ret(RET_ERRNO | EPERM));
    let jump = |from: usize, to: usize| u8::try_from(to - from - 1).expect("a short jump");
    program[x32].jt = jump(x32, deny);
    for at in sockets {
        program[at].jt = jump(at, socket_check);
    }
    for at in urings {
        program[at].jt = jump(at, deny);
    }
    for at in families {
        program[at].jt = jump(at, allow);
    }
    program
}

/// The filter as the bytes bwrap's `--seccomp` reads.
pub fn filter_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    for insn in filter() {
        out.extend_from_slice(&insn.code.to_le_bytes());
        out.push(insn.jt);
        out.push(insn.jf);
        out.extend_from_slice(&insn.k.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests;
