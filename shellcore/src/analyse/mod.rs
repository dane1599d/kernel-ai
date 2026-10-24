//! The script analyser (ADR 0002 D5): a walk over the compiled Rhai AST that finds
//! every call of a registry row, where each of its arguments came from, and what
//! decided that it runs at all. Labels are Literal < System < Content (ADR 0002
//! D6) and join through `let`, assignment, indexing, `if`, `switch`, `for` and
//! `try`; anything the walk cannot prove is Content. The analysed AST is the one
//! that runs, so a name in a string or a comment is no call, and [`Plan::allows`]
//! re-checks each live call against what was found.

use agentabi::consent::{self, Decision, Label};
use agentabi::{Action, Returns};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rhai::{AST, Expr, FnCallExpr, Position, Stmt};

#[cfg(test)]
mod tests;

/// What the walk cannot prove reads as content from this source.
pub const UNPROVEN: &str = "an unproven value";

/// Rounds a loop body is walked before the walk stops trusting its variables.
const MAX_ROUNDS: usize = 32;

/// An abstract value: where it came from, and its text when every way to it gives
/// the same literal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Val {
    pub label: Label,
    pub text: Option<String>,
}

impl Val {
    const fn of(label: Label) -> Self {
        Self { label, text: None }
    }

    fn literal(text: impl ToString) -> Self {
        Self {
            label: Label::Literal,
            text: Some(text.to_string()),
        }
    }

    fn join(&self, other: &Self) -> Self {
        Self {
            label: join(self.label, other.label),
            text: (self.text == other.text)
                .then(|| self.text.clone())
                .flatten(),
        }
    }

    /// The value assigned under `control`: it came from wherever that decision did.
    fn raised(&self, control: Label) -> Self {
        Self {
            label: join(self.label, control),
            text: self.text.clone(),
        }
    }
}

/// The less trusted of two labels; of two contents, the first.
pub const fn join(a: Label, b: Label) -> Label {
    const fn rank(label: Label) -> u8 {
        match label {
            Label::Literal => 0,
            Label::System => 1,
            Label::Content(_) => 2,
        }
    }
    if rank(b) > rank(a) { b } else { a }
}

/// One place in the script that calls a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    pub name: &'static str,
    /// The row's arguments in order, each with the value the walk found for it.
    pub args: Vec<(&'static str, Val)>,
    /// What decided that the call runs: the `if`, `for`, `switch` or early exit it
    /// sits under.
    pub control: Label,
    at: (Option<usize>, Option<usize>),
}

/// Every row call a script can make.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub sites: Vec<Site>,
}

impl Plan {
    /// The rows called, once each, in the order they are first written.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        for site in &self.sites {
            if !names.contains(&site.name) {
                names.push(site.name);
            }
        }
        names
    }

    /// Whether a live call of `name` with the arguments `live` (as text) is one
    /// the plan found: the same row, and every argument the walk knew as a literal
    /// equal to it. Values from the OS or from content were consented to as such.
    pub fn allows(&self, name: &str, live: &[&str]) -> bool {
        self.sites.iter().any(|site| {
            site.name == name
                && site.args.len() == live.len()
                && site.args.iter().zip(live).all(|((_, val), live)| {
                    val.label != Label::Literal || val.text.as_deref().is_none_or(|t| t == *live)
                })
        })
    }
}

impl Plan {
    /// Consent for every site (ADR 0002 D3, D6), from the user's `line`, in site
    /// order. A literal written as one of its argument's aliases (`"rr"`) counts as
    /// what it means, as a tagged call's does; a value whose text the walk does not
    /// know is not in any line.
    pub fn decide(&self, line: &str, rows: &[Action]) -> Vec<Decision> {
        let values: Vec<Vec<(&str, consent::Value<'_>)>> = self
            .sites
            .iter()
            .map(|site| {
                let row = agentabi::find(rows, site.name);
                site.args
                    .iter()
                    .map(|(name, val)| {
                        let text = val.text.as_deref().unwrap_or("");
                        let text = row
                            .and_then(|row| row.args.iter().find(|arg| arg.name == *name))
                            .and_then(|arg| {
                                arg.aliases.iter().find(|(written, _)| *written == text)
                            })
                            .map_or(text, |(_, means)| means);
                        (
                            *name,
                            consent::Value {
                                text,
                                label: val.label,
                            },
                        )
                    })
                    .collect()
            })
            .collect();
        let calls: Vec<consent::Call<'_>> = self
            .sites
            .iter()
            .zip(&values)
            .map(|(site, args)| consent::Call {
                name: site.name,
                args,
                control: site.control,
            })
            .collect();
        consent::decide(line, &calls, rows)
    }
}

/// The plan of `ast`, whose row calls are looked up in `rows`.
pub fn analyse(ast: &AST, rows: &'static [Action]) -> Plan {
    let mut walk = Walk {
        rows,
        env: Vec::new(),
        sites: Vec::new(),
        sticky: Label::Literal,
        seen: Label::Literal,
    };
    walk.block(ast.statements(), Label::Literal);
    Plan { sites: walk.sites }
}

type Env = Vec<(String, Val)>;

struct Walk {
    rows: &'static [Action],
    /// Variables in scope, innermost last.
    env: Env,
    sites: Vec<Site>,
    /// What decided that the rest of the script runs: an early `return`, `break`
    /// or `throw` under an `if` raises it for everything after.
    sticky: Label,
    /// The least trusted label met so far, for what a `catch` may have caught.
    seen: Label,
}

fn join_env(a: &Env, b: &Env) -> Env {
    a.iter()
        .zip(b)
        .map(|((name, x), (_, y))| (name.clone(), x.join(y)))
        .collect()
}

impl Walk {
    fn block(&mut self, stmts: &[Stmt], control: Label) -> Val {
        let depth = self.env.len();
        let mut value = Val::of(Label::Literal);
        for stmt in stmts {
            value = self.stmt(stmt, control);
        }
        self.env.truncate(depth);
        value
    }

    /// Two ways through, `first` and `second`, from the same start; the variables
    /// after are either's.
    fn branches(
        &mut self,
        first: impl FnOnce(&mut Self) -> Val,
        second: impl FnOnce(&mut Self) -> Val,
    ) -> Val {
        let start = self.env.clone();
        let one = first(self);
        let after_one = core::mem::replace(&mut self.env, start);
        let two = second(self);
        self.env = join_env(&after_one, &self.env);
        one.join(&two)
    }

    /// A body that runs any number of times, walked until its variables settle.
    fn looped(&mut self, mut body: impl FnMut(&mut Self)) {
        for _ in 0..MAX_ROUNDS {
            let before = (self.env.clone(), self.sticky);
            body(self);
            self.env = join_env(&before.0, &self.env);
            if (self.env.clone(), self.sticky) == before {
                return;
            }
        }
        // Not settled: trust nothing in it.
        let unproven = Label::Content(UNPROVEN);
        for (_, val) in &mut self.env {
            *val = Val::of(unproven);
        }
        self.sticky = unproven;
        body(self);
    }

    fn set(&mut self, name: &str, val: Val) {
        if let Some((_, slot)) = self.env.iter_mut().rev().find(|(n, _)| n == name) {
            *slot = val;
        }
    }

    fn get(&self, name: &str) -> Val {
        self.env
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map_or(Val::of(Label::Content(UNPROVEN)), |(_, val)| val.clone())
    }

    fn stmt(&mut self, stmt: &Stmt, control: Label) -> Val {
        let unit = Val::of(Label::Literal);
        match stmt {
            Stmt::Noop(_) => unit,
            Stmt::Expr(expr) => self.expr(expr, control),
            Stmt::FnCall(call, at) => self.call(call, None, *at, control),
            Stmt::Block(block) => self.block(block.statements(), control),
            Stmt::Var(var, _, _) => {
                let val = self.expr(&var.1, control).raised(control);
                self.env.push((var.0.name.to_string(), val));
                unit
            }
            Stmt::Assignment(assign) => {
                let (op, target) = (&assign.0, &assign.1);
                let rhs = self.expr(&target.rhs, control);
                if let Expr::Variable(var, _, _) = &target.lhs {
                    let val = match op.is_op_assignment() {
                        true => Val::of(join(self.get(&var.1).label, rhs.label)),
                        false => rhs,
                    };
                    self.set(&var.1, val.raised(control));
                } else if let Some(root) = root(&target.lhs) {
                    // `a[i] = v`, `a.b = v`: the whole of `a` is any of them.
                    let place = self.expr(&target.lhs, control);
                    let val = Val::of(join(join(self.get(&root).label, rhs.label), place.label));
                    self.set(&root, val.raised(control));
                }
                unit
            }
            Stmt::If(flow, _) => {
                let cond = self.expr(&flow.expr, control).label;
                let inner = join(control, cond);
                let value = self.branches(
                    |w| w.block(flow.body.statements(), inner),
                    |w| w.block(flow.branch.statements(), inner),
                );
                value.raised(cond)
            }
            Stmt::Switch(switch, _) => {
                let on = self.expr(&switch.0, control).label;
                let inner = join(control, on);
                let start = self.env.clone();
                let mut after = start.clone();
                let mut value = Val::of(Label::Literal);
                for case in &switch.1.expressions {
                    self.env = start.clone();
                    let cond = self.expr(&case.lhs, inner).label;
                    value = value.join(&self.expr(&case.rhs, join(inner, cond)));
                    after = join_env(&after, &self.env);
                }
                self.env = after;
                value.raised(on)
            }
            Stmt::For(each, _) => {
                let (var, counter, flow) = (&each.0, &each.1, &each.2);
                let over = self.expr(&flow.expr, control).label;
                let inner = join(control, over);
                self.looped(|w| {
                    let depth = w.env.len();
                    w.env.push((var.name.to_string(), Val::of(inner)));
                    if let Some(counter) = counter {
                        w.env.push((counter.name.to_string(), Val::of(inner)));
                    }
                    w.block(flow.body.statements(), inner);
                    w.env.truncate(depth);
                });
                unit
            }
            Stmt::While(flow, _) | Stmt::Do(flow, _, _) => {
                self.looped(|w| {
                    let cond = w.expr(&flow.expr, control).label;
                    w.block(flow.body.statements(), join(control, cond));
                    // Leaving the loop is the condition's decision.
                    w.sticky = join(w.sticky, cond);
                });
                unit
            }
            Stmt::TryCatch(flow, _) => {
                let seen = core::mem::replace(&mut self.seen, Label::Literal);
                let before = self.env.clone();
                let tried = self.block(flow.body.statements(), control);
                let caught = self.seen;
                self.seen = join(seen, caught);
                // The catch may start after any line of the try.
                self.env = join_env(&before, &self.env);
                let inner = join(control, caught);
                let depth = self.env.len();
                if let Expr::Variable(var, _, _) = &flow.expr {
                    self.env.push((var.1.to_string(), Val::of(inner)));
                }
                let handled = self.block(flow.branch.statements(), inner);
                self.env.truncate(depth);
                self.env = join_env(&before, &self.env);
                tried.join(&handled)
            }
            Stmt::BreakLoop(value, _, _) | Stmt::Return(value, _, _) => {
                if let Some(value) = value {
                    self.expr(value, control);
                }
                self.sticky = join(self.sticky, control);
                unit
            }
            _ => Val::of(Label::Content(UNPROVEN)),
        }
    }

    fn expr(&mut self, expr: &Expr, control: Label) -> Val {
        let val = match expr {
            Expr::BoolConstant(b, _) => Val::literal(b),
            Expr::IntegerConstant(n, _) => Val::literal(n),
            Expr::CharConstant(c, _) => Val::literal(c),
            Expr::StringConstant(s, _) => Val::literal(s),
            Expr::DynamicConstant(_, _) | Expr::Unit(_) => Val::of(Label::Literal),
            Expr::InterpolatedString(parts, _) | Expr::Array(parts, _) => {
                self.all(parts.iter(), control)
            }
            Expr::Map(map, _) => self.all(map.0.iter().map(|(_, value)| value), control),
            Expr::Variable(var, _, _) => self.get(&var.1),
            Expr::Stmt(block) => self.block(block.statements(), control),
            Expr::FnCall(call, at) => self.call(call, None, *at, control),
            Expr::Dot(pair, _, _) | Expr::Index(pair, _, _) => {
                let base = self.expr(&pair.lhs, control);
                self.chain(base, &pair.rhs, control)
            }
            Expr::And(parts, _) | Expr::Or(parts, _) | Expr::Coalesce(parts, _) => {
                // Each part runs only if the ones before let it.
                let mut acc = Val::of(Label::Literal);
                for part in parts.iter() {
                    let val = self.expr(part, join(control, acc.label));
                    acc = Val::of(join(acc.label, val.label));
                }
                acc
            }
            _ => Val::of(Label::Content(UNPROVEN)),
        };
        self.seen = join(self.seen, val.label);
        val
    }

    fn all<'a>(&mut self, parts: impl Iterator<Item = &'a Expr>, control: Label) -> Val {
        let mut label = Label::Literal;
        for part in parts {
            label = join(label, self.expr(part, control).label);
        }
        Val::of(label)
    }

    /// The rest of a `.` or `[]` chain after `acc`: a method gets everything to
    /// its left as `this`, and an index joins in.
    fn chain(&mut self, acc: Val, rhs: &Expr, control: Label) -> Val {
        match rhs {
            Expr::Dot(pair, _, _) | Expr::Index(pair, _, _) => {
                let left = self.chain(acc, &pair.lhs, control);
                self.chain(left, &pair.rhs, control)
            }
            Expr::Property(_, _) => Val::of(acc.label),
            Expr::MethodCall(call, at) => {
                let result = self.call(call, Some(acc.clone()), *at, control);
                Val::of(join(acc.label, result.label))
            }
            other => Val::of(join(acc.label, self.expr(other, control).label)),
        }
    }

    fn call(&mut self, call: &FnCallExpr, this: Option<Val>, at: Position, control: Label) -> Val {
        let mut args: Vec<Val> = this.into_iter().collect();
        for arg in call.args.iter() {
            args.push(self.expr(arg, control));
        }
        let from = args
            .iter()
            .fold(Label::Literal, |label, arg| join(label, arg.label));
        let Some(row) = self.rows.iter().find(|row| row.name == call.name.as_str()) else {
            // An operator, a helper or a built-in: pure, so its result is its
            // arguments'.
            return Val::of(from);
        };
        self.site(row, args, join(control, self.sticky), at);
        match row.returns {
            Returns::Content(source) => Val::of(Label::Content(source)),
            Returns::System => Val::of(join(Label::System, from)),
        }
    }

    /// Records a call of `row` at `at`; a call walked again (in a loop) joins.
    fn site(&mut self, row: &'static Action, args: Vec<Val>, control: Label, at: Position) {
        let at = (at.line(), at.position());
        let args: Vec<(&'static str, Val)> = args
            .into_iter()
            .enumerate()
            .map(|(i, val)| (row.args.get(i).map_or("", |arg| arg.name), val))
            .collect();
        match self
            .sites
            .iter_mut()
            .find(|s| s.at == at && s.name == row.name)
        {
            Some(site) => {
                site.control = join(site.control, control);
                for ((_, old), (_, new)) in site.args.iter_mut().zip(&args) {
                    *old = old.join(new);
                }
            }
            None => self.sites.push(Site {
                name: row.name,
                args,
                control,
                at,
            }),
        }
    }
}

/// The variable at the root of `a[i].b`, if there is one.
fn root(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(var, _, _) => Some(var.1.to_string()),
        Expr::Dot(pair, _, _) | Expr::Index(pair, _, _) => root(&pair.lhs),
        _ => None,
    }
}
