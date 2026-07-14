use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use image::RgbaImage;
use ratatui::layout::Size;
use ratatui_image::{Resize, ResizeEncodeRender, protocol::StatefulProtocol};
use rayon::prelude::*;

use crate::graphics::Graphics;

pub struct ProtocolWarmJob {
    pub epoch: u64,
    pub key: u64,
    pub source: ProtocolWarmSource,
    pub size: Size,
    pub priority: bool,
}

pub enum ProtocolWarmSource {
    Image {
        display: RgbaImage,
        graphics: Graphics,
    },
    Protocol(StatefulProtocol),
}

pub enum ProtocolWarmOutcome {
    Ready {
        epoch: u64,
        key: u64,
        size: Size,
        protocol: Box<StatefulProtocol>,
    },
    Failed {
        epoch: u64,
        key: u64,
        size: Size,
    },
    Skipped(Vec<(u64, u64, Size)>),
}

pub struct ProtocolWarmResult {
    pub outcome: ProtocolWarmOutcome,
}

pub struct ProtocolWarmWorker {
    tx: Sender<Vec<ProtocolWarmJob>>,
    rx: Receiver<ProtocolWarmResult>,
}

impl ProtocolWarmWorker {
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Vec<ProtocolWarmJob>>();
        let (result_tx, result_rx) = mpsc::channel::<ProtocolWarmResult>();
        thread::spawn(move || {
            let thread_count = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(4)
                .clamp(2, 6);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(thread_count)
                .thread_name(|index| format!("nullspace-protocol-warm-{index}"))
                .build()
                .ok();
            while let Ok(mut jobs) = job_rx.recv() {
                let mut kept = Vec::new();
                let mut skipped = Vec::new();
                while let Ok(newer_jobs) = job_rx.try_recv() {
                    for job in jobs {
                        if job.priority {
                            kept.push(job);
                        } else {
                            skipped.push(job);
                        }
                    }
                    jobs = newer_jobs;
                }
                if !kept.is_empty() {
                    kept.extend(jobs);
                    jobs = kept;
                }
                send_skipped(skipped, &result_tx);

                if let Some(pool) = &pool {
                    pool.install(|| warm_protocols(jobs, &result_tx));
                } else {
                    warm_protocols(jobs, &result_tx);
                }
            }
        });
        Self {
            tx: job_tx,
            rx: result_rx,
        }
    }

    pub fn send(&self, jobs: Vec<ProtocolWarmJob>) {
        let _ = self.tx.send(jobs);
    }

    pub fn try_recv(&self) -> Option<ProtocolWarmResult> {
        self.rx.try_recv().ok()
    }
}

fn warm_protocols(jobs: Vec<ProtocolWarmJob>, result_tx: &Sender<ProtocolWarmResult>) {
    jobs.into_par_iter()
        .for_each(|job| warm_protocol(job, result_tx));
}

fn warm_protocol(job: ProtocolWarmJob, result_tx: &Sender<ProtocolWarmResult>) {
    let epoch = job.epoch;
    let key = job.key;
    let size = job.size;
    let outcome = catch_unwind(AssertUnwindSafe(|| encode_protocol(job)))
        .unwrap_or(ProtocolWarmOutcome::Failed { epoch, key, size });
    let _ = result_tx.send(ProtocolWarmResult { outcome });
}

fn encode_protocol(job: ProtocolWarmJob) -> ProtocolWarmOutcome {
    let mut protocol = match job.source {
        ProtocolWarmSource::Image { display, graphics } => {
            graphics.protocol_from(display, job.size)
        }
        ProtocolWarmSource::Protocol(protocol) => protocol,
    };
    // Fit the image into the available area, then encode at that fitted size. The encoded
    // area is what `StatefulProtocol::needs_resize` compares against on the draw thread, so
    // it must equal `size_for(Fit, available)` — encoding at the full `job.size` instead
    // leaves the protocol perpetually "needing resize" whenever the equation doesn't fill
    // the pane, which shows an endless spinner.
    let fit_size = protocol.size_for(Resize::Fit(None), job.size);
    if fit_size.width == 0 || fit_size.height == 0 {
        return ProtocolWarmOutcome::Failed {
            epoch: job.epoch,
            key: job.key,
            size: job.size,
        };
    }
    protocol.resize_encode(&Resize::Fit(None), fit_size);
    match protocol.last_encoding_result() {
        Some(Ok(())) => ProtocolWarmOutcome::Ready {
            epoch: job.epoch,
            key: job.key,
            size: job.size,
            protocol: Box::new(protocol),
        },
        Some(Err(_)) | None => ProtocolWarmOutcome::Failed {
            epoch: job.epoch,
            key: job.key,
            size: job.size,
        },
    }
}

fn send_skipped(jobs: Vec<ProtocolWarmJob>, result_tx: &Sender<ProtocolWarmResult>) {
    let skipped = jobs
        .into_iter()
        .map(|job| (job.epoch, job.key, job.size))
        .collect::<Vec<_>>();
    if !skipped.is_empty() {
        let _ = result_tx.send(ProtocolWarmResult {
            outcome: ProtocolWarmOutcome::Skipped(skipped),
        });
    }
}
