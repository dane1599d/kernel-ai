//! The `cc` service's logic (ADR 0012 D2-D5, D11).

#![no_std]

extern crate alloc;

pub mod build;
pub mod compiler;
mod dis;
mod engine;
pub mod rows;
pub mod service;
pub mod session;

pub use compiler::{CompileError, Compiler};
pub use service::{Effects, Service};
pub use session::Session;

/// xcc's compiler, SHA-256 pinned (third_party/xcc/cc.wasm.sha256).
pub const CC_WASM: &[u8] = include_bytes!("../../third_party/xcc/cc.wasm");

/// The xcc headers session code may include, in `/kai/include`.
pub const XCC_HEADERS: [(&str, &[u8]); 9] = [
    (
        "float.h",
        include_bytes!("../../third_party/xcc/include/float.h"),
    ),
    (
        "inttypes.h",
        include_bytes!("../../third_party/xcc/include/inttypes.h"),
    ),
    (
        "limits.h",
        include_bytes!("../../third_party/xcc/include/limits.h"),
    ),
    (
        "stdarg.h",
        include_bytes!("../../third_party/xcc/include/stdarg.h"),
    ),
    (
        "stdbool.h",
        include_bytes!("../../third_party/xcc/include/stdbool.h"),
    ),
    (
        "stddef.h",
        include_bytes!("../../third_party/xcc/include/stddef.h"),
    ),
    (
        "stdint.h",
        include_bytes!("../../third_party/xcc/include/stdint.h"),
    ),
    (
        "stdnoreturn.h",
        include_bytes!("../../third_party/xcc/include/stdnoreturn.h"),
    ),
    (
        "string.h",
        include_bytes!("../../third_party/xcc/include/string.h"),
    ),
];

/// The most `cc` holds at once, in bytes, which its plan line's `pages=`
/// (`kabi::plan::CC`) must cover; a test checks it does. Fixed parts are
/// measured and rounded up; the kept modules are an estimate.
pub mod budget {
    use crate::{build, compiler, session};

    /// The `cc` program as loaded: 3.1 MiB in a dev build.
    pub const IMAGE: u64 = 4 << 20;
    /// `stack_pages=512`.
    pub const STACK: u64 = 2 << 20;
    /// cc.wasm translated, eagerly: about 1.4 MiB in S12-1.
    pub const COMPILER: u64 = 3 << 19;
    /// One kept entry's module and instance, estimated.
    pub const ENTRY: u64 = 16 << 10;
    /// A `cc build` at its height, beside the session: its objects, their
    /// copies in the link's scratch tree, its source bundle, and the linked
    /// module, its rewrite and the artifact, each at most an artifact long.
    pub const BUILD: u64 = 2 * build::MAX_OBJECTS as u64
        + livecore::artifact::MAX_SOURCE as u64
        + 3 * kabi::live::MAX_ARTIFACT as u64;

    /// The whole: the fixed parts, one compile, one session, every kept entry,
    /// and a build.
    pub const fn total() -> u64 {
        IMAGE
            + STACK
            + COMPILER
            + compiler::MAX_MEMORY as u64
            + session::SESSION_MEMORY as u64
            + ENTRY * session::MAX_ENTRIES as u64
            + BUILD
    }
}

/// What `cc build` compiles components and apps against (ADR 0012 D7):
/// `cargo xtask cc-bind`'s bindings for each world, and the `<stdlib.h>` they
/// include.
pub mod bind {
    /// One world's bindings.
    pub struct World {
        /// Its header's name, as its glue includes it.
        pub header_name: &'static str,
        pub header: &'static [u8],
        /// wit-bindgen's glue: the imports, the export wrappers, `cabi_realloc`.
        pub glue: &'static str,
        /// `<symbol> <export name>` lines, for the link: wcc reads no
        /// `export_name`.
        pub exports: &'static str,
    }

    /// `kai:app/component`.
    pub const COMPONENT: World = World {
        header_name: "component.h",
        header: include_bytes!("../kai/bind/component.h"),
        glue: include_str!("../kai/bind/component.c"),
        exports: include_str!("../kai/bind/component.exports"),
    };

    /// `kai:app/app` with an empty `actions` export.
    pub const APP: World = World {
        header_name: "app.h",
        header: include_bytes!("../kai/bind/app.h"),
        glue: include_str!("../kai/bind/app.c"),
        exports: include_str!("../kai/bind/app.exports"),
    };

    pub const STDLIB_H: &[u8] = include_bytes!("../kai/stdlib.h");
}

/// One entry's compile, and its run, each get this long (D3).
pub const LINE_NS: u64 = 2_000_000_000;

/// What `cc` asks of the process it runs in.
pub trait Host {
    /// Nanoseconds from some fixed point.
    fn now_ns(&self) -> u64;
    /// Whether Ctrl-C was pressed for the request being served.
    fn interrupted(&self) -> bool;
    /// How long one entry's compile, and its run, each get: D3's [`LINE_NS`],
    /// unless the machine is emulated and runs everything many times slower.
    fn line_ns(&self) -> u64 {
        LINE_NS
    }
}

/// Why work stopped before it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The line budget ran out.
    Deadline,
    /// Ctrl-C.
    Interrupted,
}

impl Stopped {
    /// The `trap` record's reason.
    pub fn word(self) -> &'static str {
        match self {
            Stopped::Deadline => "deadline",
            Stopped::Interrupted => "interrupted",
        }
    }
}

/// Time left for one entry's compile or run, or a whole build's compiles,
/// checked between fuel slices.
pub struct Budget<'a> {
    host: &'a dyn Host,
    deadline_ns: u64,
    /// Fuel the compiles under this budget have used, which
    /// [`compiler::MAX_FUEL`] caps should the clock fail.
    compile_fuel: u64,
}

impl<'a> Budget<'a> {
    pub fn new(host: &'a dyn Host, ns: u64) -> Self {
        Self {
            host,
            deadline_ns: host.now_ns().saturating_add(ns),
            compile_fuel: 0,
        }
    }

    pub fn now(&self) -> u64 {
        self.host.now_ns()
    }

    /// Counts `fuel` spent compiling; false once the compiles under this
    /// budget have used [`compiler::MAX_FUEL`].
    pub(crate) fn spend_compile(&mut self, fuel: u64) -> bool {
        self.compile_fuel = self.compile_fuel.saturating_add(fuel);
        self.compile_fuel < compiler::MAX_FUEL
    }

    pub fn check(&self) -> Result<(), Stopped> {
        if self.host.interrupted() {
            return Err(Stopped::Interrupted);
        }
        match self.host.now_ns() >= self.deadline_ns {
            true => Err(Stopped::Deadline),
            false => Ok(()),
        }
    }
}
