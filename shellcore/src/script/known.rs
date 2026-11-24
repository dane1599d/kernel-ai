//! Calls a `try` would hide: Rhai's `catch` catches "Function not found" too, so
//! a call of a row the script may not make (an app's row, a misspelt name) inside
//! a `try` passes the trial run, sends nothing and lets the script end ok. Every
//! call written inside a `try` or its `catch`, at any depth, must name a function
//! the strict engine has, with that many arguments, before the script is run.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use rhai::{AST, ASTNode, Engine, Expr, FnCallExpr, Stmt};

/// Names Rhai answers itself rather than through a registered function.
const KEYWORDS: [&str; 10] = [
    "print",
    "debug",
    "type_of",
    "is_def_var",
    "is_def_fn",
    "is_shared",
    "Fn",
    "call",
    "curry",
    "eval",
];

/// The first call inside a `try` that `engine` has no function for, as the
/// error the trial run would have given had nothing caught it.
pub(super) fn unknown_in_try(ast: &AST, engine: &Engine) -> Option<String> {
    let known: Vec<(String, usize)> = engine.collect_fn_metadata(
        None,
        |info| {
            Some((
                String::from(info.metadata.name.as_str()),
                info.metadata.num_params,
            ))
        },
        true,
    );
    let has = |name: &str, params: usize| {
        KEYWORDS.contains(&name) || known.iter().any(|(n, p)| n == name && *p == params)
    };
    let mut found = None;
    ast.walk(&mut |path: &[ASTNode]| {
        let in_try = path[..path.len().saturating_sub(1)]
            .iter()
            .any(|node| matches!(node, ASTNode::Stmt(Stmt::TryCatch(..))));
        let call = match path.last() {
            Some(ASTNode::Expr(Expr::FnCall(call, _)) | ASTNode::Stmt(Stmt::FnCall(call, _))) => {
                Some((call, 0))
            }
            // A method gets what it is called on as its first argument.
            Some(ASTNode::Expr(Expr::MethodCall(call, _))) => Some((call, 1)),
            _ => None,
        };
        match call {
            Some((call, this)) if in_try && !is_operator(call) => {
                let params = call.args.len() + this;
                if has(call.name.as_str(), params) {
                    return true;
                }
                found = Some(format!(
                    "Function not found: {} with {params} argument(s), inside a try, where \
                     the error would be caught and the call would never run",
                    call.name
                ));
                false
            }
            _ => true,
        }
    });
    found
}

/// An operator (`+`, `==`, `..`): Rhai runs these natively for its own types.
fn is_operator(call: &FnCallExpr) -> bool {
    call.op_token.is_some()
        || !call
            .name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
}
