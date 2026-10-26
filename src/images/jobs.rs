//! Generation jobs: each runs on its own worker thread (`backends::generate`), reports progress,
//! can be stopped, and stores its pictures straight into the library folder; the main thread
//! (`Jobs::poll`) adds them to the open library. A job's scratch folder is under the cache
//! (`image_jobs/<n>`), removed when it succeeds and kept (with Codex's log) when it fails.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::backends::{self, BackendKind, Creds, GenRequest};
use super::keychain;
use super::library::{ImageMeta, Library, name_from_prompt, now_secs, store_generated};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Running,
    Done,
    Failed,
    Stopped,
}

impl Stage {
    pub fn id(self) -> &'static str {
        match self {
            Stage::Running => "running",
            Stage::Done => "done",
            Stage::Failed => "failed",
            Stage::Stopped => "stopped",
        }
    }
}

pub struct Job {
    pub id: u64,
    pub prompt: String,
    /// "backend:model"
    pub model: String,
    pub model_name: String,
    pub count: u32,
    pub stage: Stage,
    pub note: String,
    /// library ids of the pictures it made so far
    pub images: Vec<String>,
    pub started: u64,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

enum News {
    Note(u64, String),
    Image(u64, ImageMeta),
    End(u64, Result<(), String>),
}

#[derive(Default)]
pub struct Jobs {
    pub list: Vec<Job>,
    news: Arc<Mutex<Vec<News>>>,
    next: u64,
}

/// What to make and how to file it.
pub struct Start {
    pub req: GenRequest,
    pub parent: Option<String>,
    /// library ids of the references (recorded on the pictures)
    pub ref_ids: Vec<String>,
    pub creds: Creds,
}

impl Jobs {
    pub fn running(&self) -> usize {
        self.list.iter().filter(|j| j.stage == Stage::Running).count()
    }

    pub fn get(&self, id: u64) -> Option<&Job> {
        self.list.iter().find(|j| j.id == id)
    }

    /// Starts a job; returns its id.
    pub fn start(&mut self, s: Start) -> u64 {
        self.next += 1;
        let id = self.next;
        let cancel = Arc::new(AtomicBool::new(false));
        let m = &s.req.model;
        self.list.push(Job {
            id,
            prompt: s.req.prompt.clone(),
            model: m.key(),
            model_name: m.name.clone(),
            count: s.req.count,
            stage: Stage::Running,
            note: "Starting".into(),
            images: vec![],
            started: now_secs(),
            cancel: cancel.clone(),
        });
        // only the last few finished jobs are kept in the list
        let mut finished = self.list.iter().filter(|j| j.stage != Stage::Running).count();
        self.list.retain(|j| {
            if j.stage != Stage::Running && finished > 6 {
                finished -= 1;
                false
            } else {
                true
            }
        });
        let news = self.news.clone();
        std::thread::spawn(move || work(id, s, news, cancel));
        id
    }

    /// Takes the workers' news: pictures go into `lib`; returns toasts to show.
    pub fn poll(&mut self, lib: &mut Library) -> Vec<String> {
        let news = std::mem::take(&mut *self.news.lock().unwrap_or_else(|e| e.into_inner()));
        let mut toasts = vec![];
        for n in news {
            match n {
                News::Note(id, note) => {
                    if let Some(j) = self.list.iter_mut().find(|j| j.id == id) {
                        j.note = note;
                    }
                }
                News::Image(id, meta) => {
                    if let Some(j) = self.list.iter_mut().find(|j| j.id == id) {
                        j.images.push(meta.id.clone());
                    }
                    lib.insert(meta);
                }
                News::End(id, r) => {
                    let Some(j) = self.list.iter_mut().find(|j| j.id == id) else { continue };
                    let stopped = j.cancel.load(Ordering::Relaxed);
                    match r {
                        Ok(()) if stopped => {
                            j.stage = Stage::Stopped;
                            j.note = "Stopped".into();
                        }
                        Ok(()) => {
                            j.stage = Stage::Done;
                            let n = j.images.len();
                            j.note = format!("{n} picture{} added", if n == 1 { "" } else { "s" });
                            toasts.push(format!("Images: {}", j.note));
                        }
                        Err(e) => {
                            j.stage = Stage::Failed;
                            toasts.push(format!("Images: {e}"));
                            j.note = e;
                        }
                    }
                }
            }
        }
        toasts
    }
}

fn work_dir(id: u64) -> PathBuf {
    crate::paths::cache_dir("image_jobs").join(format!("{}-{id}", std::process::id()))
}

fn work(id: u64, mut s: Start, news: Arc<Mutex<Vec<News>>>, cancel: Arc<AtomicBool>) {
    let push = |n: News| news.lock().unwrap_or_else(|e| e.into_inner()).push(n);
    let say = |t: String| push(News::Note(id, t));
    let dir = work_dir(id);
    let result = (|| -> Result<(), String> {
        let b = s.req.model.backend;
        if b != BackendKind::Codex {
            s.creds.key = keychain::get(b.key_account());
        }
        let outs = backends::generate(&s.req, &s.creds, &dir, &say, &cancel)?;
        if outs.is_empty() && !cancel.load(Ordering::Relaxed) {
            return Err("the model sent no pictures".into());
        }
        let m = &s.req.model;
        let base = name_from_prompt(&s.req.prompt);
        let n = outs.len();
        for (i, o) in outs.into_iter().enumerate() {
            let meta = ImageMeta {
                name: if n > 1 { format!("{base} {}", i + 1) } else { base.clone() },
                prompt: s.req.prompt.clone(),
                backend: m.backend.id().into(),
                model: m.id.clone(),
                seed: o.seed,
                aspect: s.req.aspect.clone(),
                parent: s.parent.clone(),
                references: s.ref_ids.clone(),
                provenance: format!("AI-generated · {}/{}", m.backend.id(), m.id),
                created: now_secs(),
                ..Default::default()
            };
            push(News::Image(id, store_generated(&o.bytes, meta)?));
        }
        Ok(())
    })();
    if result.is_ok() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    push(News::End(id, result));
}
