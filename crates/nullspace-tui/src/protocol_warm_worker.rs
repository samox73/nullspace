use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ratatui::layout::Size;
use ratatui_image::{protocol::StatefulProtocol, Resize, ResizeEncodeRender};

pub struct ProtocolWarmJob {
    pub key: u64,
    pub protocol: StatefulProtocol,
    pub size: Size,
}

pub struct ProtocolWarmResult {
    pub key: u64,
    pub outcome: ProtocolWarmOutcome,
}

pub enum ProtocolWarmOutcome {
    Ready(StatefulProtocol),
    Failed,
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
            while let Ok(jobs) = job_rx.recv() {
                for job in jobs {
                    warm_protocol(job, &result_tx);
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

fn warm_protocol(mut job: ProtocolWarmJob, result_tx: &Sender<ProtocolWarmResult>) {
    job.protocol.resize_encode(&Resize::Fit(None), job.size);
    let outcome = match job.protocol.last_encoding_result() {
        Some(Ok(())) => ProtocolWarmOutcome::Ready(job.protocol),
        Some(Err(_)) | None => ProtocolWarmOutcome::Failed,
    };
    let _ = result_tx.send(ProtocolWarmResult {
        key: job.key,
        outcome,
    });
}
