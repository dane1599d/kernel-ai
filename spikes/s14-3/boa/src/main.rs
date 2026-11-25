//! Spike S14-3: Boa 0.22 behind the shared runner.

#[path = "../../common/runner.rs"]
mod runner;

use boa_engine::builtins::promise::PromiseState;
use boa_engine::module::{ModuleLoader, ModuleRequest, Referrer};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, JsResult, JsValue, Module, NativeFunction, Source,
    js_string,
};
use runner::Thrown;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

fn main() {
    runner::main::<Boa>();
}

struct Boa {
    context: Context,
    files: Rc<Files>,
}

/// Modules from guest files, by absolute path. Boa's `SimpleModuleLoader`
/// refuses Wasm targets.
#[derive(Default)]
struct Files {
    modules: RefCell<HashMap<PathBuf, Module>>,
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

impl ModuleLoader for Files {
    async fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> JsResult<Module> {
        let specifier = request.specifier().to_std_string_escaped();
        let base = referrer
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let path = normalize(&base.join(&specifier));
        if let Some(module) = self.modules.borrow().get(&path) {
            return Ok(module.clone());
        }
        let json = request
            .attributes()
            .iter()
            .any(|a| a.key() == &js_string!("type") && a.value() == &js_string!("json"));
        let text = std::fs::read_to_string(&path).map_err(|e| {
            JsNativeError::typ().with_message(format!("cannot read {}: {e}", path.display()))
        })?;
        let module = if json {
            Module::parse_json(js_string!(text.as_str()), &mut context.borrow_mut())?
        } else {
            let source = Source::from_bytes(text.as_bytes()).with_path(&path);
            Module::parse(source, None, &mut context.borrow_mut())?
        };
        self.modules.borrow_mut().insert(path, module.clone());
        Ok(module)
    }
}

fn print(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let line = args.get_or_undefined(0).to_string(context)?;
    runner::print(line.to_std_string_escaped());
    Ok(JsValue::undefined())
}

fn eval_script(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let src = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    context.eval(Source::from_bytes(src.as_bytes()))
}

fn gc(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    boa_engine::gc::force_collect();
    Ok(JsValue::undefined())
}

impl Boa {
    fn thrown(&mut self, error: JsError) -> Thrown {
        let value = match error.into_opaque(&mut self.context) {
            Ok(value) => value,
            Err(error) => return Thrown::other(error.to_string()),
        };
        let context = &mut self.context;
        let name = value
            .as_object()
            .and_then(|o| o.get(js_string!("constructor"), context).ok())
            .and_then(|c| {
                c.as_object()
                    .and_then(|c| c.get(js_string!("name"), context).ok())
            })
            .and_then(|n| n.as_string().map(|s| s.to_std_string_escaped()))
            .unwrap_or_else(|| String::from("(not an object)"));
        let message = value
            .to_string(context)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_else(|_| String::from("(no text)"));
        Thrown { name, message }
    }
}

impl runner::Engine for Boa {
    fn new() -> Self {
        let files = Rc::new(Files::default());
        let mut context = Context::builder()
            .module_loader(files.clone())
            .build()
            .expect("a context");
        context
            .register_global_callable(js_string!("print"), 1, NativeFunction::from_fn_ptr(print))
            .expect("print");
        let global = context.global_object();
        let host = ObjectInitializer::new(&mut context)
            .function(
                NativeFunction::from_fn_ptr(eval_script),
                js_string!("evalScript"),
                1,
            )
            .function(NativeFunction::from_fn_ptr(gc), js_string!("gc"), 0)
            .property(js_string!("global"), global, Attribute::all())
            .build();
        context
            .register_global_property(js_string!("$262"), host, Attribute::all())
            .expect("$262");
        Boa { context, files }
    }

    fn script(&mut self, _name: &str, src: &str) -> Result<(), Thrown> {
        match self.context.eval(Source::from_bytes(src.as_bytes())) {
            Ok(_) => Ok(()),
            Err(error) => Err(self.thrown(error)),
        }
    }

    fn module(&mut self, path: &str, src: &str) -> Result<(), Thrown> {
        let source = Source::from_bytes(src.as_bytes()).with_path(Path::new(path));
        let module = match Module::parse(source, None, &mut self.context) {
            Ok(module) => module,
            Err(error) => return Err(self.thrown(error)),
        };
        // A test that imports itself gets this same module back.
        self.files
            .modules
            .borrow_mut()
            .insert(normalize(Path::new(path)), module.clone());
        let promise = module.load_link_evaluate(&mut self.context);
        self.run_jobs()?;
        match promise.state() {
            PromiseState::Fulfilled(_) => Ok(()),
            PromiseState::Rejected(value) => Err(self.thrown(JsError::from_opaque(value))),
            PromiseState::Pending => Err(Thrown::other("the module never settled")),
        }
    }

    fn run_jobs(&mut self) -> Result<(), Thrown> {
        match self.context.run_jobs() {
            Ok(()) => Ok(()),
            Err(error) => Err(self.thrown(error)),
        }
    }
}
