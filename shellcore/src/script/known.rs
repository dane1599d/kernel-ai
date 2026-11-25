//! Calls a `try` would hide: Rhai's `catch` catches "Function not found" too, so
//! a call of a row the script may not make (an app's row, a misspelt name) inside
//! a `try` passes the trial run, sends nothing and lets the script end ok. Every
//! call written inside a `try` or its `catch`, at any depth, must name a function
//! the strict engine has, with that many arguments, before the script is run.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::TypeId;
use core::cell::RefCell;
use rhai::{AST, ASTNode, Dynamic, Engine, Expr, FnCallExpr, Stmt};

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

/// Every function an engine has: its name and its parameters' types.
pub(super) type Known = Vec<(String, Vec<TypeId>)>;

/// The functions `engine` has, read from its function table once for both
/// checks.
pub(super) fn known(engine: &Engine) -> Rc<Known> {
    Rc::new(engine.collect_fn_metadata(
        None,
        |info| {
            Some((
                String::from(info.metadata.name.as_str()),
                info.metadata.param_types.iter().copied().collect(),
            ))
        },
        true,
    ))
}

/// The first call inside a `try` that `engine` has no function for, as the
/// error the trial run would have given had nothing caught it.
pub(super) fn unknown_in_try(ast: &AST, known: &Known) -> Option<String> {
    let has = |name: &str, params: usize| {
        KEYWORDS.contains(&name)
            || known
                .iter()
                .any(|(n, types)| n == name && types.len() == params)
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

/// The first call the trial run made that no function of `engine` takes, as
/// "Function not found", wrong argument types included: a `catch` would hide
/// it, so the run is no proof. Rhai asks the hook after any native call fails;
/// one whose name, count and types match a function failed in that function
/// (a stub row's own error), which a `catch` may well mean to catch. `print`
/// and `debug` are functions like any other here: with arguments no overload
/// takes they were not found either.
pub(super) fn watch(engine: &mut Engine, known: Rc<Known>) -> Rc<RefCell<Option<String>>> {
    let missed: Rc<RefCell<Option<String>>> = Rc::default();
    let seen = Rc::clone(&missed);
    #[allow(deprecated)] // volatile, not deprecated
    engine.on_missing_function(move |name, args, _, _| {
        let operator = !name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_');
        let found = operator
            || known
                .iter()
                .any(|(n, types)| n == name && takes(types, args));
        if !found && seen.borrow().is_none() {
            let types: Vec<&str> = args.iter().map(|arg| arg.type_name()).collect();
            *seen.borrow_mut() = Some(format!("Function not found: {name} ({})", types.join(", ")));
        }
        Ok(None)
    });
    missed
}

/// Whether a function with parameters `types` takes `args`: each one of its
/// type, or a `Dynamic` parameter. A function that ran and failed has taken its
/// by-value arguments out, leaving `()`; the first (which a method borrows) and
/// a string (which Rhai may lend as `&str`) can stay. So `()` matches only when
/// every argument after the first is `()` or a string: a call given a `()`
/// next to a number, a list or a map (a map key that is not there, as in
/// `fixed((), 0)`) was not found. A `()` among strings only cannot be told
/// apart, and still matches.
fn takes(types: &[TypeId], args: &[&mut Dynamic]) -> bool {
    let typed = |t: &TypeId, arg: &Dynamic| *t == TypeId::of::<Dynamic>() || *t == arg.type_id();
    let taken = args
        .iter()
        .skip(1)
        .all(|arg| arg.is_unit() || arg.is_string());
    types.len() == args.len()
        && types
            .iter()
            .zip(args.iter())
            .all(|(t, arg)| typed(t, arg) || (taken && arg.is_unit()))
}
