use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use image::RgbaImage;

pub struct RenderJob {
    pub generation: u64,
    pub latex: String,
    pub px: u32,
}

pub struct RenderResult {
    pub generation: u64,
    pub latex: String,
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
            while let Ok(job) = job_rx.recv() {
                let image = equivault_core::render::render_image(&job.latex, job.px);
                let _ = result_tx.send(RenderResult {
                    generation: job.generation,
                    latex: job.latex,
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
