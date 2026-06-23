use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use image::RgbaImage;

use crate::render_cache;

pub struct RenderJob {
    pub generation: u64,
    pub latex: String,
    pub px: u32,
}

pub struct RenderResult {
    pub generation: u64,
    pub latex: String,
    pub px: u32,
    pub image: Result<RgbaImage, String>,
}

pub struct RenderWorker {
    tx: Sender<RenderJob>,
    rx: Receiver<RenderResult>,
}

impl RenderWorker {
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<RenderJob>();
        let (result_tx, result_rx) = mpsc::channel::<RenderResult>();
        thread::spawn(move || {
            while let Ok(mut job) = job_rx.recv() {
                while let Ok(newer_job) = job_rx.try_recv() {
                    job = newer_job;
                }

                let image = match render_cache::load(&job.latex, job.px) {
                    Some(img) => Ok(img),
                    None => {
                        let r = nullspace_core::render::render_image(&job.latex, job.px);
                        if let Ok(img) = &r {
                            render_cache::store(&job.latex, job.px, img);
                        }
                        r
                    }
                };
                let _ = result_tx.send(RenderResult {
                    generation: job.generation,
                    latex: job.latex,
                    px: job.px,
                    image,
                });
            }
        });
        Self {
            tx: job_tx,
            rx: result_rx,
        }
    }

    pub fn send(&self, job: RenderJob) {
        let _ = self.tx.send(job);
    }

    pub fn try_recv(&self) -> Option<RenderResult> {
        self.rx.try_recv().ok()
    }
}
