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
}

/// Why work stopped before it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The 2 s line budget ran out.
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

/// Time left for one entry's compile or run, checked between fuel slices.
pub struct Budget<'a> {
    host: &'a dyn Host,
    deadline_ns: u64,
}

impl<'a> Budget<'a> {
    pub fn new(host: &'a dyn Host, ns: u64) -> Self {
        Self {
            host,
            deadline_ns: host.now_ns().saturating_add(ns),
        }
    }

    pub fn now(&self) -> u64 {
        self.host.now_ns()
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
