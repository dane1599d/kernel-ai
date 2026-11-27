//! The test host's system answers: metrics, and the `models` app's replies, as
//! the Host methods of the same names give them.

use super::*;

impl FakeHost {
    pub(super) fn fake_metrics(&self) -> Result<Metrics, kabi::Error> {
        let mut m = Metrics::empty();
        m.cpus = 4;
        m.free_frames = 256 * 100;
        m.total_frames = 256 * 128;
        m.process_count = 1;
        m.processes[0].pid = 7;
        m.processes[0].name[..3].copy_from_slice(b"llm");
        m.processes[0].name_len = 3;
        if self.lists_services.get() {
            let services = self.services.borrow();
            let running = services
                .iter()
                .enumerate()
                .filter(|(_, (_, running))| *running)
                .map(|(i, (name, _))| (20 + i as u64, name.clone()));
            let procs = self.procs.borrow();
            let plain = procs
                .iter()
                .enumerate()
                .map(|(i, name)| (30 + i as u64, name.clone()));
            for (pid, name) in running.chain(plain) {
                let slot = &mut m.processes[m.process_count as usize];
                slot.pid = pid;
                slot.name[..name.len()].copy_from_slice(name.as_bytes());
                slot.name_len = name.len() as u64;
                m.process_count += 1;
            }
        }
        m.policy = self.policy.get().unwrap_or(policy::INTERACTIVE);
        m.running_policy = m.policy;
        Ok(m)
    }

    pub(super) fn fake_models(&self, request: &str) -> Result<String, String> {
        self.model_requests.borrow_mut().push(String::from(request));
        if let Some(answer) = self.model_answer.borrow().clone() {
            return Ok(answer);
        }
        let mut m = self.models.borrow_mut();
        let words: Vec<&str> = request.split(' ').collect();
        Ok(match words.as_slice() {
            ["models"] => modelchan::listing::encode(&m),
            ["models", "import"] => String::from("ok importing fixture 4096"),
            // P5 task 9: the fixture may be downloaded; the 4B is too large
            // for this machine's memory.
            ["models", "offers"] => String::from(
                "kai-offers 1\nqwen3-4b-2507 0 stored 0 0\nfixture-dl 1 fits 0 0\nqwen3-8b 0 memory 6208 4096\n",
            ),
            ["models", "ask", "fixture-dl"] => {
                String::from("ok Download 12.0 MiB from models.kai.test")
            }
            ["models", "ask", id] => format!("err the signed models role names no {id}"),
            ["models", "download", "fixture-dl"] => {
                String::from("ok downloading fixture-dl 12587009 models.kai.test")
            }
            ["models", "use", hex] => {
                let sha = modelchan::listing::sha256(hex)?;
                match (m.store.running == Some(sha), m.next() == Some(sha)) {
                    (_, true) => String::from("ok already"),
                    (true, false) => {
                        m.store.next = None;
                        String::from("ok cancelled")
                    }
                    (false, false) => {
                        m.store.next = Some(sha);
                        format!("ok next {hex}")
                    }
                }
            }
            // The undo of a model_fetch: refused while the blob runs or is on
            // trial, as `updated` refuses it.
            ["models", "remove", hex] => {
                let sha = modelchan::listing::sha256(hex)?;
                if m.store.running == Some(sha) {
                    String::from("err it is the model running; use another first")
                } else if m.next() == Some(sha) {
                    String::from("err it is on trial for the next restart; undo that first")
                } else {
                    m.store.blobs.retain(|b| b.sha256 != sha);
                    String::from("ok removed")
                }
            }
            _ => String::from("err unknown request"),
        })
    }
}
