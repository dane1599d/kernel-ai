//! Spike S14-3: rquickjs 0.14 (QuickJS-NG) behind the shared runner.

#[path = "../../common/runner.rs"]
mod runner;

use rquickjs::context::EvalOptions;
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::promise::PromiseState;
use rquickjs::{Context, Ctx, Error, Function, Module, Object, Runtime, Value};
use runner::Thrown;
use std::path::{Component, Path, PathBuf};

fn main() {
    runner::main::<Qjs>();
}

struct Qjs {
    runtime: Runtime,
    context: Context,
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Module names are absolute guest paths, resolved against the importer's.
struct Files;

impl Resolver for Files {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        let dir = Path::new(base).parent().unwrap_or(Path::new("/"));
        Ok(normalize(&dir.join(name)).to_string_lossy().into_owned())
    }
}

impl Loader for Files {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        let text = std::fs::read_to_string(name).map_err(|_| Error::new_loading(name))?;
        let json = attributes
            .and_then(|a| a.get_type().ok().flatten())
            .is_some_and(|t| t == "json");
        let src = if json {
            format!("export default {text};")
        } else {
            text
        };
        Module::declare(ctx.clone(), name, src)
    }
}

fn thrown(ctx: &Ctx<'_>, error: Error) -> Thrown {
    if !error.is_exception() {
        return Thrown::other(error.to_string());
    }
    let value: Value = ctx.catch();
    let name = value
        .as_object()
        .and_then(|o| o.get::<_, Object>("constructor").ok())
        .and_then(|c| c.get::<_, String>("name").ok())
        .unwrap_or_else(|| String::from("(not an object)"));
    let message = ctx
        .globals()
        .get::<_, Function>("String")
        .and_then(|f| f.call::<_, String>((value,)))
        .unwrap_or_else(|_| String::from("(no text)"));
    Thrown { name, message }
}

/// `$262.evalScript`: a global script in this realm.
fn eval_script<'js>(ctx: Ctx<'js>, src: String) -> rquickjs::Result<Value<'js>> {
    let mut options = EvalOptions::default();
    options.global = true;
    ctx.eval_with_options(src, options)
}

impl runner::Engine for Qjs {
    fn new() -> Self {
        let runtime = Runtime::new().expect("a runtime");
        // The guest's stack is 8 MiB (.cargo/config.toml); stop well short.
        runtime.set_max_stack_size(4 << 20);
        runtime.set_loader(Files, Files);
        let context = Context::full(&runtime).expect("a context");
        context.with(|ctx| {
            let globals = ctx.globals();
            let print = Function::new(ctx.clone(), |line: rquickjs::Coerced<String>| {
                runner::print(line.0);
            })
            .expect("print");
            globals.set("print", print).expect("print");
            let host = Object::new(ctx.clone()).expect("$262");
            let eval = Function::new(ctx.clone(), eval_script).expect("evalScript");
            host.set("evalScript", eval).expect("evalScript");
            host.set("gc", Function::new(ctx.clone(), || {}).expect("gc"))
                .expect("gc");
            host.set("global", globals.clone()).expect("global");
            globals.set("$262", host).expect("$262");
        });
        Qjs { runtime, context }
    }

    fn script(&mut self, name: &str, src: &str) -> Result<(), Thrown> {
        self.context.with(|ctx| {
            let mut options = EvalOptions::default();
            options.global = true;
            options.strict = false;
            options.filename = Some(name.to_string());
            ctx.eval_with_options::<Value, _>(src, options)
                .map(|_| ())
                .map_err(|e| thrown(&ctx, e))
        })
    }

    fn module(&mut self, path: &str, src: &str) -> Result<(), Thrown> {
        let promise_failed = self.context.with(|ctx| {
            Module::evaluate(ctx.clone(), path, src)
                .map(|p| rquickjs::Persistent::save(&ctx, p))
                .map_err(|e| thrown(&ctx, e))
        });
        let promise = promise_failed?;
        self.run_jobs()?;
        self.context.with(|ctx| {
            let promise = promise.restore(&ctx).map_err(|e| thrown(&ctx, e))?;
            match promise.state() {
                PromiseState::Resolved => Ok(()),
                PromiseState::Rejected => match promise.result::<Value>() {
                    Some(Err(e)) => Err(thrown(&ctx, e)),
                    _ => Err(Thrown::other("the module was rejected")),
                },
                PromiseState::Pending => Err(Thrown::other("the module never settled")),
            }
        })
    }

    fn run_jobs(&mut self) -> Result<(), Thrown> {
        loop {
            match self.runtime.execute_pending_job() {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(_) => return Err(Thrown::other("a job threw")),
            }
        }
    }
}
