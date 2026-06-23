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
    pub latex: String,
    pub px: u32,
    pub outcome: WarmOutcome,
}

pub enum WarmOutcome {
    Ready(Result<RgbaImage, String>),
    Skipped,
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
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .thread_name(|index| format!("nullspace-render-warm-{index}"))
                .build()
                .ok();

            while let Ok(mut jobs) = job_rx.recv() {
                while let Ok(newer_jobs) = job_rx.try_recv() {
                    send_skipped(jobs, &result_tx);
                    jobs = newer_jobs;
                }

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
            latex: job.latex,
            px: job.px,
            outcome: WarmOutcome::Ready(image),
        });
    });
}

fn send_skipped(jobs: Vec<WarmJob>, result_tx: &Sender<WarmResult>) {
    for job in jobs {
        let _ = result_tx.send(WarmResult {
            latex: job.latex,
            px: job.px,
            outcome: WarmOutcome::Skipped,
        });
    }
}
