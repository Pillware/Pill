use core::panic;
use std::collections::HashMap;
use std::time::Instant;
use anyhow::{Result, Error};
use indexmap::IndexMap;
use crate::error::EngineError::*;

#[derive(Clone)]
#[readonly::make]
pub struct TimerRecord {
    #[readonly]
    pub duration: f32, // in milliseconds
    #[readonly]
    pub subrecords: IndexMap<String, TimerRecord>,
    last_subrecord_time: Instant,
}

impl TimerRecord {
    pub fn new() -> Self {
        Self {
            duration: 0.0,
            subrecords: IndexMap::new(),
            last_subrecord_time: Instant::now(),
        }
    }

    pub fn record(&mut self, label: &str) -> Result<()> {
        // Update previous record duration
        if let Some((_, previous_record)) = self.subrecords.last_mut() {
            previous_record.duration = self.last_subrecord_time.elapsed().as_secs_f32() * 1000.0;
        }

        // Create new record
        self.subrecords.insert(label.to_string(), 
        TimerRecord {
                duration: 0.0,
                subrecords: IndexMap::new(),
                last_subrecord_time: Instant::now(),
            }
        );

        self.last_subrecord_time = Instant::now();
        Ok(())
    }

    pub fn end(&mut self) -> Result<()> {
        // Update the duration of the last record
        if let Some((_, last_record)) = self.subrecords.last_mut() {
            last_record.duration = last_record.last_subrecord_time.elapsed().as_secs_f32() * 1000.0;
            // Update the duration of the current record by summing up all subrecords
            self.duration = self.subrecords.values().map(|r| r.duration).sum();
        } else {
            // If no subrecords exist, the duration is set to the elapsed time since the start
            self.duration = self.last_subrecord_time.elapsed().as_secs_f32() * 1000.0;
        }

        Ok(())
    }
}

#[derive(Clone)]
#[readonly::make]
pub struct Timer {
    #[readonly]
    pub records: IndexMap<String, TimerRecord>,
    current_context_path: Vec<String>,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            records: IndexMap::new(),
            current_context_path: Vec::new(),
        }
    }

    pub fn get_total_duration(&self) -> f32 {
        self.records.values().map(|record| record.duration).sum()
    }

    // Start new recording and switches context to it so that all subsequent records will be added to it
    pub fn record_new_context(&mut self, label: &str) -> Result<()> {
        let mut current_context_records = &mut self.records;

        // Navigate to the current context
        for current_context_path_part in &self.current_context_path {
            current_context_records = &mut current_context_records.get_mut(current_context_path_part).unwrap().subrecords;
        }

        // Before switching to new context, end task in the previous context
        if let Some(last_context) = self.current_context_path.last() {
            if let Some(last_record) = current_context_records.get_mut(last_context) {
                last_record.end()?;
            }
        }

        // Create or get the record for the current context
        current_context_records.entry(label.to_string()).or_insert_with(TimerRecord::new);
        self.current_context_path.push(label.to_string());

        Ok(())
    }

    pub fn end_context(&mut self) -> Result<()> {
        // Ensure we have a context to end
        if self.current_context_path.is_empty() {
            return Err(NoTimerContextToEnd().into());
        }

        self.get_active_context()?.end()?;
        self.current_context_path.pop();

        Ok(())
    }

    pub fn record(&mut self, label: &str) -> Result<()> {
        self.get_active_context()?.record(label)
    }

    fn get_active_context(&mut self) -> Result<&mut TimerRecord> {
        if self.current_context_path.is_empty() {
            panic!("No active timer context to get");
        }

        let mut current_context_records = &mut self.records;

        for current_context_path_part in self.current_context_path.iter().take(self.current_context_path.len() - 1) {
            current_context_records = &mut current_context_records
                .get_mut(current_context_path_part)
                .ok_or_else(|| Error::msg(format!("Missing context '{}'", current_context_path_part)))?
                .subrecords;
        }

        let context_label = self.current_context_path.last().ok_or_else(|| Error::msg("No active context"))?;
        Ok(current_context_records.get_mut(context_label).ok_or_else(|| Error::msg("Missing current context"))?)
    }
}



