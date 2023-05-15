use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use arc_swap::ArcSwapOption;
use metrics::Key;
use parking_lot::Mutex;
use pin_project::pin_project;
use quanta::Instant;
use serde::Serialize;
use thread_local::ThreadLocal;

#[cfg(feature = "unstable-stuck-detection")]
use crate::stuck_detection::StuckDetector;
use crate::ActorMeta;

static BUSY_TIME_SECONDS: Key = Key::from_static_name("elfo_busy_time_seconds");
static ALLOCATED_BYTES: Key = Key::from_static_name("elfo_allocated_bytes_total");
static DEALLOCATED_BYTES: Key = Key::from_static_name("elfo_deallocated_bytes_total");

#[derive(Serialize)]
pub struct TraceRecord {
    pub meta: Arc<ActorMeta>,
    pub start: u64,
    pub end: u64,
}

struct Tracer {
    traces: ThreadLocal<Mutex<Vec<TraceRecord>>>,
    now: Instant,
}

static TRACE: ArcSwapOption<Tracer> = ArcSwapOption::const_empty();

pub fn set_trace() {
    TRACE.store(Some(Arc::new(Tracer {
        traces: ThreadLocal::new(),
        now: Instant::now(),
    })));
}

pub fn take_traces() -> Option<Vec<TraceRecord>> {
    let tracer = Arc::try_unwrap(TRACE.swap(None)?).ok()?;
    Some(
        tracer
            .traces
            .into_iter()
            .flat_map(|t| t.into_inner())
            .collect(),
    )
}

#[pin_project]
pub(crate) struct MeasurePoll<F> {
    #[pin]
    inner: F,
    #[cfg(feature = "unstable-stuck-detection")]
    stuck_detector: StuckDetector,
}

impl<F> MeasurePoll<F> {
    #[cfg(not(feature = "unstable-stuck-detection"))]
    pub(crate) fn new(inner: F) -> Self {
        Self { inner }
    }

    #[cfg(feature = "unstable-stuck-detection")]
    pub(crate) fn new(inner: F, stuck_detector: StuckDetector) -> Self {
        Self {
            inner,
            stuck_detector,
        }
    }
}

impl<F: Future> Future for MeasurePoll<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let start = TRACE.load().as_ref().map(|t| t.now.elapsed());
        #[cfg(feature = "unstable-stuck-detection")]
        this.stuck_detector.enter();
        let result = if let Some(recorder) = metrics::try_recorder() {
            let start_time = Instant::now();
            let res = this.inner.poll(cx);
            let elapsed = Instant::now().duration_since(start_time);
            recorder.record_histogram(&BUSY_TIME_SECONDS, elapsed.as_secs_f64());
            crate::scope::with(|scope| {
                recorder.increment_counter(&ALLOCATED_BYTES, scope.take_allocated_bytes() as u64);
                recorder
                    .increment_counter(&DEALLOCATED_BYTES, scope.take_deallocated_bytes() as u64);
            });
            res
        } else {
            this.inner.poll(cx)
        };

        #[cfg(feature = "unstable-stuck-detection")]
        this.stuck_detector.exit();
        if let Some(start) = start {
            if let Some(tracer) = &*TRACE.load() {
                let traces = tracer.traces.get_or_default();
                traces.lock().push(TraceRecord {
                    meta: crate::scope::meta(),
                    start: start.as_nanos() as u64,
                    end: tracer.now.elapsed().as_nanos() as u64,
                });
            }
        }

        result
    }
}
