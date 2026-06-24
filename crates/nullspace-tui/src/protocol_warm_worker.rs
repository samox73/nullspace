use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ratatui::layout::Size;
use ratatui_image::{protocol::StatefulProtocol, Resize, ResizeEncodeRender};
use rayon::prelude::*;

pub struct ProtocolWarmJob {
    pub key: u64,
    pub protocol: StatefulProtocol,
    pub size: Size,
    pub priority: bool,
}

pub enum ProtocolWarmOutcome {
    Ready {
        key: u64,
        protocol: Box<StatefulProtocol>,
    },
    Failed {
        key: u64,
    },
    Skipped(Vec<u64>),
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

fn warm_protocol(mut job: ProtocolWarmJob, result_tx: &Sender<ProtocolWarmResult>) {
    job.protocol.resize_encode(&Resize::Fit(None), job.size);
    let outcome = match job.protocol.last_encoding_result() {
        Some(Ok(())) => ProtocolWarmOutcome::Ready {
            key: job.key,
            protocol: Box::new(job.protocol),
        },
        Some(Err(_)) | None => ProtocolWarmOutcome::Failed { key: job.key },
    };
    let _ = result_tx.send(ProtocolWarmResult { outcome });
}

fn send_skipped(jobs: Vec<ProtocolWarmJob>, result_tx: &Sender<ProtocolWarmResult>) {
    let keys = jobs.into_iter().map(|job| job.key).collect::<Vec<_>>();
    if !keys.is_empty() {
        let _ = result_tx.send(ProtocolWarmResult {
            outcome: ProtocolWarmOutcome::Skipped(keys),
        });
    }
}
