use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use image::RgbaImage;

use crate::render_cache;

pub struct QueueJob {
    pub key: u64,
    pub latex: String,
    pub px: u32,
    pub priority: u8,
}

pub struct QueueResult {
    pub key: u64,
    #[allow(dead_code)]
    pub latex: String,
    #[allow(dead_code)]
    pub px: u32,
    pub image: Result<RgbaImage, String>,
}

pub struct RenderQueue {
    tx: Sender<QueueJob>,
    rx: Receiver<QueueResult>,
}

impl RenderQueue {
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<QueueJob>();
        let (result_tx, result_rx) = mpsc::channel::<QueueResult>();

        thread::spawn(move || {
            let thread_count = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(4)
                .clamp(2, 6);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(thread_count)
                .thread_name(|index| format!("nullspace-render-{index}"))
                .build()
                .ok();

            let (done_tx, done_rx) = mpsc::channel::<u64>();

            let mut heap: BinaryHeap<PendingJob> = BinaryHeap::new();
            let mut heap_keys: HashSet<u64> = HashSet::new();
            let mut inflight: HashSet<u64> = HashSet::new();
            // Number of concurrent rayon tasks we allow at once.
            let max_concurrent = thread_count;
            let mut inflight_count: usize = 0;

            loop {
                // Drain incoming jobs into the heap (deduplication).
                loop {
                    match job_rx.try_recv() {
                        Ok(job) => {
                            if !heap_keys.contains(&job.key) && !inflight.contains(&job.key) {
                                heap_keys.insert(job.key);
                                heap.push(PendingJob {
                                    key: job.key,
                                    latex: job.latex,
                                    px: job.px,
                                    priority: job.priority,
                                });
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }

                // Drain completed task notifications.
                while let Ok(key) = done_rx.try_recv() {
                    inflight.remove(&key);
                    inflight_count = inflight_count.saturating_sub(1);
                }

                // Dispatch pending jobs to rayon up to max_concurrent.
                while inflight_count < max_concurrent {
                    let Some(job) = heap.pop() else {
                        break;
                    };
                    heap_keys.remove(&job.key);
                    inflight.insert(job.key);
                    inflight_count += 1;

                    let result_tx = result_tx.clone();
                    let done_tx = done_tx.clone();
                    let key = job.key;
                    let latex = job.latex;
                    let px = job.px;

                    let dispatch = move || {
                        // artificial delay to test list/equation decoupling
                        // std::thread::sleep(std::time::Duration::from_secs(10));
                        let image = match render_cache::load(&latex, px) {
                            Some(img) => Ok(img),
                            None => {
                                let r = nullspace_core::render::render_image(&latex, px);
                                if let Ok(img) = &r {
                                    render_cache::store(&latex, px, img);
                                }
                                r
                            }
                        };
                        let _ = result_tx.send(QueueResult {
                            key,
                            latex,
                            px,
                            image,
                        });
                        let _ = done_tx.send(key);
                    };

                    match &pool {
                        Some(p) => {
                            p.spawn(dispatch);
                        }
                        None => {
                            thread::spawn(dispatch);
                        }
                    }
                }

                // If heap is empty and nothing is inflight, block waiting for the next job.
                if heap.is_empty() && inflight_count == 0 {
                    match job_rx.recv() {
                        Ok(job) => {
                            if !heap_keys.contains(&job.key) && !inflight.contains(&job.key) {
                                heap_keys.insert(job.key);
                                heap.push(PendingJob {
                                    key: job.key,
                                    latex: job.latex,
                                    px: job.px,
                                    priority: job.priority,
                                });
                            }
                        }
                        Err(_) => return,
                    }
                } else {
                    // Brief yield to avoid busy-spinning when inflight tasks are running.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });

        Self {
            tx: job_tx,
            rx: result_rx,
        }
    }

    pub fn submit(&self, job: QueueJob) {
        let _ = self.tx.send(job);
    }

    pub fn try_recv(&self) -> Option<QueueResult> {
        self.rx.try_recv().ok()
    }
}

/// A job sitting in the coordinator's `BinaryHeap`. Lower `priority` value means
/// higher urgency (0 = selected item). The heap is a max-heap, so we invert the
/// ordering so that the smallest priority value is popped first.
struct PendingJob {
    key: u64,
    latex: String,
    px: u32,
    priority: u8,
}

impl PartialEq for PendingJob {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.key == other.key
    }
}

impl Eq for PendingJob {}

impl PartialOrd for PendingJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // Invert: lower priority value → higher urgency → comes out of the heap first.
        other
            .priority
            .cmp(&self.priority)
            .then(other.key.cmp(&self.key))
    }
}
