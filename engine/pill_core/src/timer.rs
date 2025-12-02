use std::time::Instant;

use anyhow::{Context, Error, Result};
use indexmap::IndexMap;

#[derive(Debug, Clone)]
pub struct TimerRecord {
    pub label: String,
    pub duration: f32, // ms
    pub subrecords: Vec<TimerRecord>,
}

#[derive(Debug, Clone)]
struct ActiveContext {
    label: String,
    start_time: Instant,
    subrecords: Vec<TimerRecord>,
    last_split_time: Instant,
}

#[derive(Debug, Clone)]
pub struct Timer {
    stack: Vec<ActiveContext>,
    pub records: Vec<TimerRecord>,
    current_label: Option<String>,
    current_label_start: Option<Instant>,
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            records: Vec::new(),
            current_label: None,
            current_label_start: None,
        }
    }

    pub fn begin_context(&mut self, label: impl Into<String>) {
        // Close current segment if needed
        self.flush_record();

        let label = label.into();
        let now = Instant::now();
        self.stack.push(ActiveContext {
            label,
            start_time: now,
            last_split_time: now,
            subrecords: Vec::new(),
        });
    }

    pub fn end_context(&mut self) -> Result<()> {
        self.flush_record(); // End any remaining record

        let finished = self.stack.pop().context("No context to end")?;
        let duration = finished.start_time.elapsed().as_secs_f32() * 1000.0;
        let record = TimerRecord {
            label: finished.label,
            duration,
            subrecords: finished.subrecords,
        };

        if let Some(parent) = self.stack.last_mut() {
            parent.subrecords.push(record);
        } else {
            self.records.push(record);
        }

        Ok(())
    }

    pub fn record(&mut self, label: impl Into<String>) {
        self.flush_record(); // End any previous one
        self.current_label = Some(label.into());
        self.current_label_start = Some(Instant::now());
    }

    fn flush_record(&mut self) {
        if let (Some(label), Some(start), Some(current)) = (
            &self.current_label,
            self.current_label_start,
            self.stack.last_mut(),
        ) {
            let duration = start.elapsed().as_secs_f32() * 1000.0;
            current.subrecords.push(TimerRecord {
                label: label.clone(),
                duration,
                subrecords: Vec::new(),
            });
        }

        self.current_label = None;
        self.current_label_start = None;
    }

    pub fn total_duration(&self) -> f32 {
        self.records.iter().map(|r| r.duration).sum()
    }

    pub fn print(&self, indent: usize) {
        for record in &self.records {
            Self::print_record(record, indent);
        }
    }

    fn print_record(record: &TimerRecord, indent: usize) {
        println!(
            "{:indent$}- {}: {:.3}ms",
            "",
            record.label,
            record.duration,
            indent = indent
        );
        for sub in &record.subrecords {
            Self::print_record(sub, indent + 2);
        }
    }
}
