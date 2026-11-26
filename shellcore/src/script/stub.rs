//! The host a script's trial run uses: every read answered as the real one
//! would, and no change made.

#[allow(unused_imports)]
use super::*;

/// Stand-in for the OS during the trial run. Nothing changes, and the time, heap and
/// Ctrl-C checks use the real host, so the trial is bounded like any other run.
/// Its numbers are all non-zero and its list is as long as the real one can be, with
/// names as long as real names can be, so an index or a division that works on the
/// real system works here too.
pub(super) struct Stub(pub(super) Rc<dyn Host>);

impl Host for Stub {
    fn now_ns(&self) -> u64 {
        self.0.now_ns()
    }
    fn interrupted(&self) -> bool {
        self.0.interrupted()
    }
    fn heap_bytes(&self) -> u64 {
        self.0.heap_bytes()
    }
    fn metrics(&self) -> Result<Metrics, kabi::Error> {
        let mut m = Metrics::empty();
        m.cpus = 2;
        m.free_frames = 256 * 64;
        m.total_frames = 256 * 128;
        m.heap_bytes = 64 * 1024;
        m.uptime_ns = 1_000_000_000;
        m.context_switches = 1;
        m.ejections = 1;
        m.migrations = 1;
        for (index, slot) in m.processes.iter_mut().enumerate() {
            slot.pid = index as u64 + 2;
            slot.name = [b'x'; kabi::MAX_NAME];
            slot.name_len = kabi::MAX_NAME as u64;
        }
        m.process_count = MAX_METRIC_PROCESSES as u64;
        Ok(m)
    }
    fn set_policy(&self, _: u64) -> Result<(), kabi::Error> {
        Ok(())
    }
    fn stop_process(&self, _: &str, _: Proof) -> Result<u64, String> {
        Ok(1)
    }
    fn stop_service(&self, _: &str, _: Proof) -> Result<(), String> {
        Ok(())
    }
    fn start_service(&self, _: &str, _: u64) -> Result<(), String> {
        Ok(())
    }

    fn restart_service(&self, _: &str, _: Proof) -> Result<(), String> {
        Ok(())
    }
    fn fs_read(&self, _: FsRead) -> Result<String, String> {
        Ok(String::new())
    }
    fn fs_change(&self, _: FsChange) -> Result<u64, String> {
        Ok(0)
    }
    // A trial reaches no host: the network rows answer as an empty network.
    fn net_fetch(&self, _: &str, _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn net_resolve(&self, _: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    fn net_ping(&self, _: &str) -> Result<i64, String> {
        Ok(-1)
    }
    /// An answer in the kind's shape, with no model asked: the trial checks
    /// the script, not what the data says.
    fn query(&self, request: &llmproto::query::QueryRequest) -> Result<String, String> {
        use llmproto::query::QueryKind;
        Ok(match &request.kind {
            QueryKind::Bool => String::from("yes"),
            QueryKind::Enum(words) => words.first().cloned().unwrap_or_default(),
            QueryKind::Short => String::from("an answer"),
        })
    }
    fn apps(&self) -> Option<&core::cell::RefCell<crate::apps::Apps>> {
        self.0.apps()
    }
    fn withheld_read(&self) -> Result<Option<String>, String> {
        self.0.withheld_read()
    }
    // A trial withholds nothing: the file is the one copy.
    fn withheld_write(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn app_call(&self, _: &str, _: &str) -> Result<String, String> {
        Ok(alloc::format!("{}\n", livecore::call::REPLY_HEADER))
    }
    fn appearance_read(&self) -> Option<String> {
        self.0.appearance_read()
    }
    fn appearance_write(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn ui(&self, call: &crate::ui::UiCall) -> Result<String, String> {
        use crate::ui::UiCall;
        Ok(String::from(match call {
            UiCall::Windows => "w1\tx\tx\t1",
            UiCall::Focus(_) => "none",
            UiCall::Close(window) => return Ok(window.clone()),
            _ => "",
        }))
    }
    fn ask(&self, _: &str) -> Result<String, String> {
        Err("not available to scripts".into())
    }
    fn script(
        &self,
        _: &llmproto::script::ScriptRequest,
    ) -> Result<Answer, llmproto::script::AnswerError> {
        Err(llmproto::script::AnswerError::Server(
            "not available to scripts".into(),
        ))
    }
    fn audit(&self, _: &str) {}
}
