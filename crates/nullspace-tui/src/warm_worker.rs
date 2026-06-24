use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use image::RgbaImage;
use rayon::prelude::*;

use crate::render_cache;

#[derive(Clone)]
pub struct WarmJob {
    pub latex: String,
    pub px: u32,
}

pub struct WarmResult {
    pub outcome: WarmOutcome,
}

pub enum WarmOutcome {
    Ready {
        latex: String,
        px: u32,
        image: Result<RgbaImage, String>,
    },
    Skipped(Vec<WarmJob>),
}

pub struct WarmWorker {
    tx: Sender<Vec<WarmJob>>,
    rx: Receiver<WarmResult>,
}

impl WarmWorker {
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Vec<WarmJob>>();
        let (result_tx, result_rx) = mpsc::channel::<WarmResult>();
        thread::spawn(move || {
            let thread_count = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(4)
                .clamp(2, 6);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(thread_count)
                .thread_name(|index| format!("nullspace-render-warm-{index}"))
                .build()
                .ok();

            while let Ok(mut jobs) = job_rx.recv() {
                let mut skipped = Vec::new();
                while let Ok(newer_jobs) = job_rx.try_recv() {
                    skipped.extend(jobs);
                    jobs = newer_jobs;
                }
                send_skipped(skipped, &result_tx);

                if let Some(pool) = &pool {
                    pool.install(|| warm_jobs(jobs, &result_tx));
                } else {
                    warm_jobs(jobs, &result_tx);
                }
            }
        });
        Self {
            tx: job_tx,
            rx: result_rx,
        }
    }

    pub fn send(&self, jobs: Vec<WarmJob>) {
        let _ = self.tx.send(jobs);
    }

    pub fn try_recv(&self) -> Option<WarmResult> {
        self.rx.try_recv().ok()
    }
}

fn warm_jobs(jobs: Vec<WarmJob>, result_tx: &Sender<WarmResult>) {
    jobs.into_par_iter().for_each(|job| {
        let image = match render_cache::load(&job.latex, job.px) {
            Some(img) => Ok(img),
            None => {
                let rendered = nullspace_core::render::render_image(&job.latex, job.px);
                if let Ok(img) = &rendered {
                    render_cache::store(&job.latex, job.px, img);
                }
                rendered
            }
        };
        let _ = result_tx.send(WarmResult {
            outcome: WarmOutcome::Ready {
                latex: job.latex,
                px: job.px,
                image,
            },
        });
    });
}

fn send_skipped(jobs: Vec<WarmJob>, result_tx: &Sender<WarmResult>) {
    if !jobs.is_empty() {
        let _ = result_tx.send(WarmResult {
            outcome: WarmOutcome::Skipped(jobs),
        });
    }
}
