//! Repeated numerical measurements through the `metrics` crate.
//!
//! # Responsibilities
//!
//! - Install one process-wide [`metrics::Recorder`] for the engine.
//! - Forward gauges and histograms to Tracy plots when the `tracy` feature is
//!   active, so repeated numbers appear as native Tracy graphs.
//!
//! # Design
//!
//! The host's telemetry setup calls [`install_metrics`] once. After that, the
//! `metrics::gauge!`, `metrics::counter!` and `metrics::histogram!` macros
//! record into this recorder from any crate.
//!
//! The recorder keeps no copy of the values. A gauge `set` or a histogram
//! sample reaches a Tracy plot when a profiler is connected, and goes nowhere
//! otherwise, so nothing in the engine reads a metric back. Without an
//! installed recorder the `metrics` macros fall back to a no-op one, so
//! installing this recorder is what makes their values go anywhere. Plot
//! names are cached, so the leaked `PlotName` is created once per metric name.

// Standard library
#[cfg(feature = "tracy")]
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(feature = "tracy")]
use std::sync::{Mutex, Weak};

// External crates
use metrics::{Counter, Gauge, Histogram, Key, KeyName, Recorder, SharedString, Unit};

// =============================================================================
// Types
// =============================================================================

/// The state shared by the recorder and the handles it hands out: the cache
/// of plot names, and the flag that says whether forwarding to Tracy is on.
#[cfg(feature = "tracy")]
struct TracyForwardingState {
    plot_names_by_metric_name: Mutex<HashMap<String, tracy_client::PlotName>>,
    tracy_forwarding_enabled: bool,
}

#[cfg(feature = "tracy")]
impl TracyForwardingState {
    /// Pushes one point into a native Tracy plot when a profiler is connected.
    fn emit_tracy_plot(&self, metric_name: &str, sample_value: f64) {
        // Step 1: Stop unless forwarding is on and a profiler is running.
        // The client is asked a second time below, because a profiler can
        // disconnect between the two calls.
        if !self.tracy_forwarding_enabled || !tracy_client::Client::is_running() {
            return;
        }
        let Some(client) = tracy_client::Client::running() else {
            return;
        };
        // Step 2: Get the plot name from the cache, creating it on first use.
        // `PlotName::new_leak` leaks one string per distinct metric name, so
        // the cache keeps that leak to one per name.
        let mut plot_names_by_metric_name = self
            .plot_names_by_metric_name
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let plot_name = match plot_names_by_metric_name.get(metric_name) {
            Some(existing) => *existing,
            None => {
                let created = tracy_client::PlotName::new_leak(sanitize_plot_name(metric_name));
                plot_names_by_metric_name.insert(metric_name.to_owned(), created);
                created
            }
        };
        // Step 3: Push the sample into the Tracy plot.
        client.plot(plot_name, sample_value);
    }
}

/// The process-wide recorder: accepts every `metrics` update and forwards the
/// ones Tracy can plot.
///
/// A gauge `set` and a histogram sample reach a Tracy plot when the `tracy`
/// feature is on and a profiler is connected. Gauge increments, gauge
/// decrements and every counter update are dropped.
///
/// It implements the full [`Recorder`] surface because the `metrics` crate
/// requires one. It keeps no store: the only state it holds is the plot-name
/// cache Tracy needs.
#[derive(Clone)]
pub struct EngineMetricsRecorder {
    #[cfg(feature = "tracy")]
    tracy_forwarding_state: Arc<TracyForwardingState>,
}

impl EngineMetricsRecorder {
    /// Creates a recorder with an empty plot-name cache.
    ///
    /// Without the `tracy` feature there is no cache, because nothing is
    /// forwarded.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "tracy")]
            tracy_forwarding_state: Arc::new(TracyForwardingState {
                plot_names_by_metric_name: Mutex::new(HashMap::new()),
                tracy_forwarding_enabled: true,
            }),
        }
    }

    /// Installs this recorder as the process-wide metrics recorder.
    ///
    /// # Errors
    ///
    /// Returns the recorder back when another recorder was already installed.
    pub fn try_install(self) -> Result<(), EngineMetricsRecorder> {
        metrics::set_global_recorder(self).map_err(|error| error.into_inner())
    }
}

impl Default for EngineMetricsRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder for EngineMetricsRecorder {
    fn describe_counter(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}
    fn describe_gauge(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}
    fn describe_histogram(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn register_counter(&self, key: &Key, _metadata: &metrics::Metadata<'_>) -> Counter {
        // Counters never reach a Tracy plot, so the counter handle drops every
        // update whether the feature is on or off.
        let _ = key;
        Counter::from_arc(Arc::new(NullHandle))
    }

    fn register_gauge(&self, key: &Key, _metadata: &metrics::Metadata<'_>) -> Gauge {
        #[cfg(feature = "tracy")]
        {
            Gauge::from_arc(Arc::new(TracyPlotGauge {
                metric_name: key.name().to_owned(),
                tracy_forwarding_state: Arc::downgrade(&self.tracy_forwarding_state),
            }))
        }
        #[cfg(not(feature = "tracy"))]
        {
            let _ = key;
            Gauge::from_arc(Arc::new(NullHandle))
        }
    }

    fn register_histogram(&self, key: &Key, _metadata: &metrics::Metadata<'_>) -> Histogram {
        #[cfg(feature = "tracy")]
        {
            Histogram::from_arc(Arc::new(TracyPlotHistogram {
                metric_name: key.name().to_owned(),
                tracy_forwarding_state: Arc::downgrade(&self.tracy_forwarding_state),
            }))
        }
        #[cfg(not(feature = "tracy"))]
        {
            let _ = key;
            Histogram::from_arc(Arc::new(NullHandle))
        }
    }
}

/// A handle that drops every update.
///
/// Counters get this handle in every build, and gauges and histograms get it
/// when the `tracy` feature is off. The recorder keeps no copy of the values,
/// so an update here is a call that does nothing. The handle exists because
/// `metrics` asks for one every time a metric is registered.
struct NullHandle;

impl metrics::CounterFn for NullHandle {
    fn increment(&self, _value: u64) {}
    fn absolute(&self, _value: u64) {}
}

impl metrics::GaugeFn for NullHandle {
    fn increment(&self, _value: f64) {}
    fn decrement(&self, _value: f64) {}
    fn set(&self, _value: f64) {}
}

impl metrics::HistogramFn for NullHandle {
    fn record(&self, _value: f64) {}
    fn record_many(&self, _value: f64, _count: usize) {}
}

/// The gauge handle that sends every `set` value to a Tracy plot.
#[cfg(feature = "tracy")]
struct TracyPlotGauge {
    metric_name: String,
    tracy_forwarding_state: Weak<TracyForwardingState>,
}

#[cfg(feature = "tracy")]
impl metrics::GaugeFn for TracyPlotGauge {
    fn increment(&self, _value: f64) {}
    fn decrement(&self, _value: f64) {}
    fn set(&self, value: f64) {
        if let Some(tracy_forwarding_state) = self.tracy_forwarding_state.upgrade() {
            tracy_forwarding_state.emit_tracy_plot(&self.metric_name, value);
        }
    }
}

/// The histogram handle that sends every recorded sample to a Tracy plot.
#[cfg(feature = "tracy")]
struct TracyPlotHistogram {
    metric_name: String,
    tracy_forwarding_state: Weak<TracyForwardingState>,
}

#[cfg(feature = "tracy")]
impl metrics::HistogramFn for TracyPlotHistogram {
    fn record(&self, value: f64) {
        if let Some(tracy_forwarding_state) = self.tracy_forwarding_state.upgrade() {
            tracy_forwarding_state.emit_tracy_plot(&self.metric_name, value);
        }
    }
    fn record_many(&self, value: f64, count: usize) {
        if let Some(tracy_forwarding_state) = self.tracy_forwarding_state.upgrade() {
            for _ in 0..count {
                tracy_forwarding_state.emit_tracy_plot(&self.metric_name, value);
            }
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Turns a metric name into the name of its Tracy plot.
///
/// Every character that is not an ASCII letter, a digit or an underscore
/// becomes an underscore, so `engine.frame_time_ms` becomes
/// `engine_frame_time_ms`.
#[cfg(feature = "tracy")]
fn sanitize_plot_name(metric_name: &str) -> String {
    metric_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Installs the engine metrics recorder process-wide.
///
/// Returns `true` when the recorder was installed and `false` when another
/// recorder was already installed, which then stays in place.
pub fn install_metrics() -> bool {
    install_metrics_with(EngineMetricsRecorder::new())
}

/// Installs a specific engine metrics recorder process-wide.
///
/// [`install_metrics`] is this function with a new [`EngineMetricsRecorder`].
/// Returns `true` when the recorder was installed and `false` when another
/// recorder was already installed.
pub fn install_metrics_with(recorder: EngineMetricsRecorder) -> bool {
    recorder.try_install().is_ok()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Every handle accepts every update, and nothing panics.
    ///
    /// The recorder keeps no copy of the values: a gauge `set` or a histogram
    /// sample goes to a Tracy plot when a profiler is attached, and nowhere
    /// otherwise. There is therefore no stored state for this test to read
    /// back, so what it checks is that registering a handle and updating it
    /// never panics.
    #[test]
    fn handles_accept_every_update_and_keep_nothing() {
        use metrics::Level;
        let recorder = EngineMetricsRecorder::new();
        let metadata = metrics::Metadata::new("test", Level::INFO, None);

        let gauge = recorder.register_gauge(&Key::from_name("test.gauge"), &metadata);
        gauge.set(7.0);
        gauge.increment(1.0);
        gauge.decrement(0.5);

        let histogram = recorder.register_histogram(&Key::from_name("test.histogram"), &metadata);
        histogram.record(1.5);
        histogram.record_many(1.5, 3);

        let counter = recorder.register_counter(&Key::from_name("test.counter"), &metadata);
        counter.increment(3);
        counter.absolute(9);
    }

    /// Sanitizing a metric name turns its punctuation into underscores.
    #[cfg(feature = "tracy")]
    #[test]
    fn plot_names_are_sanitized_into_identifiers() {
        assert_eq!(
            sanitize_plot_name("engine.frame_time_ms"),
            "engine_frame_time_ms"
        );
        assert_eq!(sanitize_plot_name("ecs.entities"), "ecs_entities");
    }
}
