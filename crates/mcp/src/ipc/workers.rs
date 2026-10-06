use std::thread::JoinHandle;

use anyhow::{Context, Result, ensure};

pub(super) const MAX_IN_FLIGHT: usize = 4;

#[derive(Default)]
pub(super) struct Workers(Vec<JoinHandle<()>>);

impl Workers {
    pub fn start(&mut self, work: impl FnOnce() + Send + 'static) -> Result<()> {
        let mut index = 0;
        while index < self.0.len() {
            if self.0[index].is_finished() {
                let _ = self.0.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        ensure!(self.0.len() < MAX_IN_FLIGHT, "REQUEST_LIMIT: at most four requests may be in flight");
        self.0.push(
            std::thread::Builder::new()
                .name("capopen-ipc-request".into())
                .spawn(work)
                .context("IPC_UNAVAILABLE: starting request worker")?,
        );
        Ok(())
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        for worker in self.0.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn in_flight_workers_are_bounded() {
        let mut workers = Workers::default();
        let mut releases = Vec::new();
        for _ in 0..MAX_IN_FLIGHT {
            let (tx, rx) = mpsc::channel();
            releases.push(tx);
            workers
                .start(move || {
                    rx.recv().unwrap();
                })
                .unwrap();
        }
        assert!(workers.start(|| panic!("must not run")).unwrap_err().to_string().starts_with("REQUEST_LIMIT"));
        for tx in releases {
            tx.send(()).unwrap();
        }
        drop(workers);
    }
}
