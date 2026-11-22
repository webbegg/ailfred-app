//! The one thread that writes conversations: the main thread sends it what to save and goes on.
//! Jobs run in the order they were sent. It also keeps the list of the most recent
//! conversations, and says when that changes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use super::store::{patch_title, write_snapshot};
use super::{Entry, RECENT, Snapshot, entry_of, recent};

enum Job {
    Save(Snapshot),
    /// An automatic title for a conversation that isn't the open chat's.
    Title { id: String, title: String },
    /// What was on disk at launch.
    Seed(Vec<Entry>),
    Flush(Sender<()>),
}

#[derive(Clone)]
pub struct Writer {
    jobs: Sender<Job>,
}

impl Writer {
    /// Starts the thread, writing into `dir` (made on the first save). `on_recent` hears the
    /// most recent conversations (at most `RECENT`) whenever that list changes, on this thread.
    pub fn start(dir: PathBuf, on_recent: impl Fn(Vec<Entry>) + Send + 'static) -> Writer {
        let (jobs, inbox) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            let mut known: HashMap<String, Entry> = HashMap::new();
            let mut told: Vec<Entry> = Vec::new();
            for job in inbox {
                match job {
                    Job::Save(snapshot) => match write_snapshot(&dir, &snapshot) {
                        Ok(()) => {
                            if let Some(entry) = entry_of(&snapshot.saved.id, &snapshot.saved) {
                                known.insert(entry.id.clone(), entry);
                            }
                        }
                        Err(e) => eprintln!("Ailfred: saving conversation {} failed: {e}", snapshot.saved.id),
                    },
                    Job::Title { id, title } => match patch_title(&dir, &id, &title) {
                        Ok(Some(entry)) => _ = known.insert(id, entry),
                        Ok(None) => {}
                        Err(e) => eprintln!("Ailfred: titling conversation {id} failed: {e}"),
                    },
                    // What was saved since is newer than what the scan saw.
                    Job::Seed(entries) => {
                        for entry in entries {
                            known.entry(entry.id.clone()).or_insert(entry);
                        }
                    }
                    Job::Flush(done) => {
                        let _ = done.send(());
                        continue;
                    }
                }
                let top = recent(known.values().cloned().collect(), RECENT);
                if top != told {
                    on_recent(top.clone());
                    told = top;
                }
            }
        });
        Writer { jobs }
    }

    pub fn save(&self, snapshot: Snapshot) {
        let _ = self.jobs.send(Job::Save(snapshot));
    }

    pub fn set_title(&self, id: String, title: String) {
        let _ = self.jobs.send(Job::Title { id, title });
    }

    /// The conversations the launch's scan found.
    pub fn seed(&self, entries: Vec<Entry>) {
        let _ = self.jobs.send(Job::Seed(entries));
    }

    /// Waits (at most `wait`) until everything sent so far is written; whether it was.
    pub fn flush(&self, wait: Duration) -> bool {
        let (done, waiting) = mpsc::channel();
        self.jobs.send(Job::Flush(done)).is_ok() && waiting.recv_timeout(wait).is_ok()
    }
}
