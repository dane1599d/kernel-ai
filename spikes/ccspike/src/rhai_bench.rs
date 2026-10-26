//! The sieve and the sum in Rhai, for measure (e): 1.27.0 on the AST interpreter
//! and on Grain. (The tree moved from 1.26.1 to 1.27.0, and cargo cannot hold two
//! 1.x versions of one crate, so 1.26.1 is not measured.) Engines have no operation limit, so the
//! times are the loops' own.

use crate::Clock;
use alloc::format;
use alloc::string::String;

/// Which Rhai runs a script.
#[derive(Clone, Copy, Debug)]
pub enum Rhai {
    Ast127,
    Grain127,
}

impl Rhai {
    pub const ALL: [Rhai; 2] = [Rhai::Ast127, Rhai::Grain127];

    pub fn name(self) -> &'static str {
        match self {
            Rhai::Ast127 => "rhai 1.27.0 AST",
            Rhai::Grain127 => "rhai 1.27.0 Grain",
        }
    }
}

/// Runs `script`; returns its integer value and the nanoseconds the run took
/// (compiling the script excluded).
pub fn run(rhai: Rhai, script: &str, clock: Clock) -> Result<(i64, u64), String> {
    match rhai {
        Rhai::Ast127 | Rhai::Grain127 => {
            let engine = rhai::Engine::new();
            let ast = engine.compile(script).map_err(|e| format!("{e}"))?;
            let start = clock();
            let value: i64 = if matches!(rhai, Rhai::Grain127) {
                let program = rhai::grain::Compiler::new().compile(&ast);
                let mut scope = rhai::Scope::new();
                rhai::grain::Vm::new(&engine)
                    .eval_with_scope(&mut scope, &program)
                    .map_err(|e| format!("{e}"))?
                    .as_int()
                    .map_err(|t| format!("not an integer: {t}"))?
            } else {
                engine.eval_ast(&ast).map_err(|e| format!("{e}"))?
            };
            Ok((value, clock() - start))
        }
    }
}
