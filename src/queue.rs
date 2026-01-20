use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Hotkey,
    Auto,
}

#[derive(Debug, Clone)]
pub struct HotkeyJob {
    pub audio_path: PathBuf,
    pub text_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AutoJob {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub processed_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum Job {
    Hotkey(HotkeyJob),
    Auto(AutoJob),
}

#[derive(Debug)]
pub struct JobQueue {
    hotkey_session_active: bool,
    pending_hotkey: Option<HotkeyJob>,
    auto_queue: VecDeque<AutoJob>,
    active: Option<JobKind>,
}

impl JobQueue {
    pub fn new() -> Self {
        Self {
            hotkey_session_active: false,
            pending_hotkey: None,
            auto_queue: VecDeque::new(),
            active: None,
        }
    }

    pub fn begin_hotkey_session(&mut self) -> bool {
        if self.hotkey_session_active {
            return false;
        }
        self.hotkey_session_active = true;
        true
    }

    pub fn cancel_hotkey_session(&mut self) {
        self.hotkey_session_active = false;
        self.pending_hotkey = None;
        if self.active == Some(JobKind::Hotkey) {
            self.active = None;
        }
    }

    pub fn hotkey_session_active(&self) -> bool {
        self.hotkey_session_active
    }

    pub fn enqueue_hotkey(&mut self, job: HotkeyJob) -> bool {
        if self.pending_hotkey.is_some() {
            return false;
        }
        self.pending_hotkey = Some(job);
        true
    }

    pub fn enqueue_auto(&mut self, job: AutoJob) {
        self.auto_queue.push_back(job);
    }

    pub fn next_job(&mut self) -> Option<Job> {
        if self.active.is_some() {
            return None;
        }
        if let Some(job) = self.pending_hotkey.take() {
            self.active = Some(JobKind::Hotkey);
            return Some(Job::Hotkey(job));
        }
        if self.hotkey_session_active {
            return None;
        }
        if let Some(job) = self.auto_queue.pop_front() {
            self.active = Some(JobKind::Auto);
            return Some(Job::Auto(job));
        }
        None
    }

    pub fn active_kind(&self) -> Option<JobKind> {
        self.active
    }

    pub fn complete_active(&mut self, kind: JobKind) {
        if self.active == Some(kind) {
            self.active = None;
        }
        if kind == JobKind::Hotkey {
            self.hotkey_session_active = false;
        }
    }
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_blocks_auto_until_complete() {
        let mut queue = JobQueue::new();
        let auto_job = AutoJob {
            input_path: PathBuf::from("in.m4a"),
            output_path: PathBuf::from("out.md"),
            processed_path: PathBuf::from("processed.m4a"),
        };
        queue.enqueue_auto(auto_job);
        assert!(queue.begin_hotkey_session());
        assert!(queue.next_job().is_none());

        let hotkey_job = HotkeyJob {
            audio_path: PathBuf::from("rec.m4a"),
            text_path: PathBuf::from("rec.md"),
        };
        assert!(queue.enqueue_hotkey(hotkey_job));
        assert!(matches!(queue.next_job(), Some(Job::Hotkey(_))));
        queue.complete_active(JobKind::Hotkey);
        assert!(matches!(queue.next_job(), Some(Job::Auto(_))));
    }

    #[test]
    fn hotkey_priority_over_auto() {
        let mut queue = JobQueue::new();
        queue.enqueue_auto(AutoJob {
            input_path: PathBuf::from("in.m4a"),
            output_path: PathBuf::from("out.md"),
            processed_path: PathBuf::from("processed.m4a"),
        });
        assert!(queue.begin_hotkey_session());
        assert!(queue.enqueue_hotkey(HotkeyJob {
            audio_path: PathBuf::from("rec.m4a"),
            text_path: PathBuf::from("rec.md"),
        }));
        assert!(matches!(queue.next_job(), Some(Job::Hotkey(_))));
    }

    #[test]
    fn hotkey_session_rejects_second_start() {
        let mut queue = JobQueue::new();
        assert!(queue.begin_hotkey_session());
        assert!(!queue.begin_hotkey_session());
    }
}
