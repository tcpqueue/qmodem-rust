//! Bounded scheduling metadata; command payloads and response contents stay out.
use super::*;
use std::collections::VecDeque;
use std::sync::{Mutex as StdMutex, MutexGuard};

#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: u64,
    pub modem_id: Option<String>,
    pub operation: &'static str,
    pub queued_ms: u64,
    pub elapsed_ms: Option<u64>,
    pub commands_started: usize,
    pub last_command: Option<&'static str>,
    pub caller_detached: Option<bool>,
    pub outcome: Option<&'static str>,
}
struct Entry {
    id: u64,
    modem_id: Option<String>,
    operation: &'static str,
    submitted: Instant,
    started: Option<Instant>,
    commands: usize,
    last_command: Option<&'static str>,
}
impl Entry {
    fn view(&self) -> JobView {
        JobView {
            id: self.id,
            modem_id: self.modem_id.clone(),
            operation: self.operation,
            queued_ms: self
                .started
                .unwrap_or_else(Instant::now)
                .duration_since(self.submitted)
                .as_millis() as u64,
            elapsed_ms: self.started.map(|s| s.elapsed().as_millis() as u64),
            commands_started: self.commands,
            last_command: self.last_command,
            caller_detached: None,
            outcome: None,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct QueueView {
    pub state: &'static str,
    pub capacity: usize,
    pub waiting_count: usize,
    pub current: Option<JobView>,
    pub waiting: Vec<JobView>,
    pub recent: Vec<JobView>,
    pub completed: u64,
    pub failed: u64,
    pub cancelled_before_start: u64,
    pub rejected_queue_full: u64,
}
struct Data {
    accepting: bool,
    next_id: u64,
    state: &'static str,
    current: Option<Entry>,
    waiting: VecDeque<Entry>,
    recent: VecDeque<JobView>,
    completed: u64,
    failed: u64,
    cancelled: u64,
    rejected: u64,
}
#[derive(Clone)]
pub(super) struct Monitor(Arc<StdMutex<Data>>);
impl Default for Monitor {
    fn default() -> Self {
        Self(Arc::new(StdMutex::new(Data {
            accepting: true,
            next_id: 1,
            state: "idle",
            current: None,
            waiting: VecDeque::new(),
            recent: VecDeque::new(),
            completed: 0,
            failed: 0,
            cancelled: 0,
            rejected: 0,
        })))
    }
}
impl Monitor {
    fn lock(&self) -> MutexGuard<'_, Data> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn submit(
        &self,
        sender: &mpsc::Sender<Job>,
        program: Box<dyn Program>,
        reply: oneshot::Sender<std::result::Result<Vec<Reply>, AtError>>,
        modem_id: Option<String>,
        operation: &'static str,
    ) -> std::result::Result<(), AtError> {
        let mut d = self.lock();
        if !d.accepting {
            return Err(err(ErrorKind::Closed, "AT port is closed"));
        }
        let id = d.next_id;
        d.next_id = d.next_id.wrapping_add(1);
        // Hold metadata lock until try_send completes so dequeue cannot overtake registration.
        match sender.try_send(Job { id, program, reply }) {
            Ok(()) => {
                d.waiting.push_back(Entry {
                    id,
                    modem_id,
                    operation,
                    submitted: Instant::now(),
                    started: None,
                    commands: 0,
                    last_command: None,
                });
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                d.rejected += 1;
                Err(err(ErrorKind::QueueFull, "AT queue is full"))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(err(ErrorKind::Closed, "AT port is closed"))
            }
        }
    }
    fn remember(d: &mut Data, entry: JobView) {
        if d.recent.len() == 32 {
            d.recent.pop_back();
        }
        d.recent.push_front(entry);
    }
    pub fn begin(&self, id: u64, cancelled: bool) {
        let mut d = self.lock();
        let Some(index) = d.waiting.iter().position(|e| e.id == id) else {
            return;
        };
        let mut entry = d.waiting.remove(index).unwrap();
        if cancelled {
            d.cancelled += 1;
            let mut view = entry.view();
            view.outcome = Some("cancelled_before_start");
            view.caller_detached = Some(true);
            Self::remember(&mut d, view);
        } else {
            entry.started = Some(Instant::now());
            d.current = Some(entry);
            d.state = "running";
        }
    }
    pub fn phase(&self, phase: &'static str) {
        self.lock().state = phase;
    }
    pub fn command(&self, label: &'static str) {
        let mut d = self.lock();
        d.state = "running";
        if let Some(e) = d.current.as_mut() {
            e.commands += 1;
            e.last_command = Some(label);
        }
    }
    pub fn finish(&self, outcome: &'static str, detached: bool) {
        let mut d = self.lock();
        if let Some(entry) = d.current.take() {
            let mut view = entry.view();
            view.outcome = Some(outcome);
            view.caller_detached = Some(detached);
            if outcome == "completed" {
                d.completed += 1;
            } else {
                d.failed += 1;
            }
            Self::remember(&mut d, view);
        }
        d.state = "idle";
    }
    pub fn freeze(&self) -> Result<()> {
        let mut d = self.lock();
        ensure!(
            d.current.is_none() && d.waiting.is_empty() && d.state != "recovering",
            "port is busy; wait for all transactions and recovery to finish"
        );
        d.accepting = false;
        Ok(())
    }
    pub fn close(&self) {
        let mut d = self.lock();
        d.state = "closed";
        d.accepting = false;
        while let Some(entry) = d.waiting.pop_front() {
            let mut view = entry.view();
            view.outcome = Some("port_closed");
            d.failed += 1;
            Self::remember(&mut d, view);
        }
    }
    pub fn snapshot(&self) -> QueueView {
        let d = self.lock();
        QueueView {
            state: d.state,
            capacity: 32,
            waiting_count: d.waiting.len(),
            current: d.current.as_ref().map(Entry::view),
            waiting: d.waiting.iter().map(Entry::view).collect(),
            recent: d.recent.iter().cloned().collect(),
            completed: d.completed,
            failed: d.failed,
            cancelled_before_start: d.cancelled,
            rejected_queue_full: d.rejected,
        }
    }
}
