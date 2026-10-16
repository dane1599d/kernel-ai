//! M0 spike: prove Rhai and wasmi build for x86_64-unknown-none and run on bare metal.
//! Both run on the kernel heap.

use crate::log;

/// Runs both spikes and logs their results. Returns an error message on failure.
pub fn run() -> Result<(), &'static str> {
    match spike_rhai::run() {
        Ok(42) => log!("spike-rhai: ok (script returned 42)"),
        Ok(other) => {
            log!("spike-rhai: wrong result {}", other);
            return Err("Rhai spike returned the wrong value");
        }
        Err(error) => {
            log!("spike-rhai: error {}", error);
            return Err("Rhai spike failed");
        }
    }
    match spike_wasmi::run() {
        Ok(42) => log!("spike-wasmi: ok (add(40, 2) returned 42)"),
        Ok(other) => {
            log!("spike-wasmi: wrong result {}", other);
            return Err("wasmi spike returned the wrong value");
        }
        Err(error) => {
            log!("spike-wasmi: error {}", error);
            return Err("wasmi spike failed");
        }
    }
    Ok(())
}
