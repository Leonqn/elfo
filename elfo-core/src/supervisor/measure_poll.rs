use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use arc_swap::ArcSwapOption;
use fxhash::FxHashMap;
use metrics::Key;
use opentelemetry::{
    global::{self, BoxedTracer},
    trace::{TraceContextExt, Tracer},
};
use parking_lot::Mutex;
use pin_project::pin_project;
use quanta::Instant;

#[cfg(feature = "unstable-stuck-detection")]
use crate::stuck_detection::StuckDetector;

static BUSY_TIME_SECONDS: Key = Key::from_static_name("elfo_busy_time_seconds");
static ALLOCATED_BYTES: Key = Key::from_static_name("elfo_allocated_bytes_total");
static DEALLOCATED_BYTES: Key = Key::from_static_name("elfo_deallocated_bytes_total");

#[derive(Default)]
struct GroupTrace {
    span: Option<opentelemetry::Context>,
    actors_count: usize,
}

static PARENT_SPAN: ArcSwapOption<(
    BoxedTracer,
    opentelemetry::Context,
    FxHashMap<String, Mutex<GroupTrace>>,
)> = ArcSwapOption::const_empty();

pub fn set_span(span_name: String, groups: impl Iterator<Item = String>) {
    let tracer = global::tracer("");
    let span = opentelemetry::Context::current_with_span(tracer.start(span_name));
    let groups = groups
        .map(|group| (group, Default::default()))
        .collect::<FxHashMap<_, _>>();
    PARENT_SPAN.store(Some(Arc::new((tracer, span, groups))));
}

pub fn remove_span() {
    PARENT_SPAN.store(None);
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

        let span = PARENT_SPAN
            .load()
            .as_deref()
            .and_then(|(tracer, span, groups)| {
                let meta = crate::scope::meta();
                let group = groups.get(&meta.group)?;
                let mut gg = group.lock();
                if gg.actors_count == 0 {
                    gg.span = Some(opentelemetry::Context::current_with_span(
                        tracer.start_with_context(meta.group.clone(), span),
                    ))
                }
                gg.actors_count += 1;
                let span = tracer
                    .start_with_context(meta.key.clone(), gg.span.as_ref().expect("must present"));
                Some(opentelemetry::trace::mark_span_as_active(span))
            });

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
        drop(span);
        PARENT_SPAN.load().as_deref().and_then(|(_, _, groups)| {
            let meta = crate::scope::meta();
            let group = groups.get(&meta.group)?;
            let mut gg = group.lock();
            if gg.actors_count != 0 {
                gg.actors_count -= 1;
            }
            if gg.actors_count == 0 {
                gg.span = None;
            }
            Some(())
        });
        result
    }
}
