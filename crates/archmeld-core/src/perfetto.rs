// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Native Perfetto trace protobuf adapter.
//!
//! Perfetto traces are protobuf (`Trace.packet`). This decoder reads only the
//! fields needed to reconstruct explicit observations:
//! * `TrackEvent` slices (begin/end/instant) become spans;
//! * `TrackEvent` counters become counter series;
//! * `TrackEvent` states become closed state intervals (the final open state
//!   is reported as a loss, never closed with a fabricated timestamp);
//! * clock snapshots calibrate packet timestamps into one declared clock
//!   domain; cross-snapshot drift is recorded.
//!
//! The adapter never invents causality: slices get no parent, counters and
//! states never create links, and source attribution comes only from an
//! explicit `archmeld.entity_id` debug annotation or an exact source location
//! supplied by the caller. Unsupported packet kinds are counted as losses.

use crate::error::{CoreError, Result};
use crate::trace_adapters::{base_trace_with_metrics, SpanDraft, TraceLocator, TraceMeta};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MAX_PACKETS: usize = 5_000_000;
const MAX_EVENTS: usize = 1_000_000;
const SLICE_BEGIN: u64 = 1;
const SLICE_END: u64 = 2;
const INSTANT: u64 = 3;
const COUNTER: u64 = 4;
const STATE: u64 = 5;

pub fn perfetto_trace(bytes: &[u8], locator: &TraceLocator, meta: &TraceMeta) -> Result<Value> {
    let mut state = Decoder::default();
    let mut fields = Fields::new(bytes);
    while let Some((number, wire)) = fields.next_field()? {
        if number != 1 {
            continue;
        }
        let Wire::Bytes(packet) = wire else {
            return Err(CoreError::invalid("Trace.packet must be length-delimited"));
        };
        state.packet(packet, locator)?;
        if state.truncated {
            break;
        }
    }
    state.finish(meta)
}

#[derive(Default)]
struct Decoder {
    sequences: HashMap<u64, SequenceState>,
    clocks: Clocks,
    multipliers: HashMap<u32, i128>,
    root_clock: Option<u32>,
    tracks: HashMap<u64, TrackInfo>,
    open_slices: HashMap<u64, Vec<OpenSlice>>,
    open_states: HashMap<u64, OpenState>,
    incremental_counters: HashMap<u64, i128>,
    counters: BTreeMap<u64, CounterSeries>,
    states: BTreeMap<u64, StateSeries>,
    spans: Vec<SpanDraft>,
    notes: BTreeSet<String>,
    unknown_clocks: BTreeSet<u32>,
    packets: usize,
    events: usize,
    truncated: bool,
    unclosed_slices: usize,
}

#[derive(Default)]
struct SequenceState {
    timestamp_accum: i128,
    last_event_us: i128,
    clock_id: Option<u32>,
    default_track: Option<u64>,
    names: HashMap<u64, String>,
    categories: HashMap<u64, String>,
    debug_names: HashMap<u64, String>,
}

#[derive(Default, Clone)]
struct TrackInfo {
    name: Option<String>,
    process_id: Option<String>,
    thread_id: Option<String>,
    is_counter: bool,
    counter_incremental: bool,
    unit: Option<String>,
    is_state: bool,
}

struct OpenSlice {
    start_ns: i128,
    name: String,
    category: Option<String>,
    entity_id: Option<String>,
    attributes: Map<String, Value>,
    process_id: Option<String>,
    thread_id: Option<String>,
}

struct OpenState {
    start_ns: i128,
    value: Value,
}

#[derive(Default)]
struct CounterSeries {
    name: String,
    unit: Option<String>,
    entity_id: Option<String>,
    attributes: Map<String, Value>,
    samples: Vec<Value>,
}

#[derive(Default)]
struct StateSeries {
    name: String,
    entity_id: Option<String>,
    attributes: Map<String, Value>,
    intervals: Vec<Value>,
}

struct Packet<'a> {
    timestamp: Option<u64>,
    clock_id: Option<u32>,
    sequence: Option<u64>,
    flags: u32,
    snapshot: Option<&'a [u8]>,
    interned: Option<&'a [u8]>,
    defaults: Option<&'a [u8]>,
    descriptor: Option<&'a [u8]>,
    event: Option<&'a [u8]>,
}

impl Decoder {
    fn packet(&mut self, data: &[u8], locator: &TraceLocator) -> Result<()> {
        self.packets += 1;
        if self.packets > MAX_PACKETS {
            self.truncated = true;
            self.notes.insert(format!(
                "trace truncated after {MAX_PACKETS} packets; later packets were ignored"
            ));
            return Ok(());
        }
        let mut packet = Packet {
            timestamp: None,
            clock_id: None,
            sequence: None,
            flags: 0,
            snapshot: None,
            interned: None,
            defaults: None,
            descriptor: None,
            event: None,
        };
        let mut fields = Fields::new(data);
        while let Some((number, wire)) = fields.next_field()? {
            match number {
                8 => packet.timestamp = Some(wire.varint()?),
                58 => packet.clock_id = Some(wire.varint()? as u32),
                10 => packet.sequence = Some(wire.varint()?),
                13 => packet.flags = wire.varint()? as u32,
                6 => packet.snapshot = Some(wire.bytes()?),
                12 => packet.interned = Some(wire.bytes()?),
                59 => packet.defaults = Some(wire.bytes()?),
                60 => packet.descriptor = Some(wire.bytes()?),
                11 => packet.event = Some(wire.bytes()?),
                _ => {}
            }
        }
        let sequence_id = packet.sequence.unwrap_or(0);
        let cleared = packet.flags & 1 != 0;
        {
            let sequence = self.sequences.entry(sequence_id).or_default();
            if cleared {
                *sequence = SequenceState::default();
            }
            if let Some(defaults) = packet.defaults {
                parse_defaults(defaults, sequence)?;
            }
            if let Some(interned) = packet.interned {
                parse_interned(interned, sequence)?;
            }
            if let Some(timestamp) = packet.timestamp {
                if packet.flags & 2 != 0 {
                    sequence.timestamp_accum = sequence
                        .timestamp_accum
                        .checked_add(i128::from(timestamp))
                        .ok_or_else(|| CoreError::invalid("packet timestamp overflow"))?;
                } else {
                    sequence.timestamp_accum = i128::from(timestamp);
                }
            }
        }
        if let Some(snapshot) = packet.snapshot {
            self.parse_clock_snapshot(snapshot)?;
        }
        if let Some(descriptor) = packet.descriptor {
            self.parse_track(descriptor)?;
        }
        if let Some(event) = packet.event {
            self.parse_event(event, sequence_id, packet.clock_id, locator)?;
        }
        Ok(())
    }

    fn parse_clock_snapshot(&mut self, data: &[u8]) -> Result<()> {
        let mut clocks: Vec<(u32, i128)> = Vec::new();
        let mut fields = Fields::new(data);
        while let Some((number, wire)) = fields.next_field()? {
            if number != 1 {
                continue;
            }
            let entry = wire.bytes()?;
            let mut id = None;
            let mut timestamp = 0i128;
            let mut multiplier = 1i128;
            let mut clock_fields = Fields::new(entry);
            while let Some((number, wire)) = clock_fields.next_field()? {
                match number {
                    1 => id = Some(wire.varint()? as u32),
                    2 => timestamp = i128::from(wire.varint()?),
                    4 => {
                        let value = wire.varint()?;
                        if value > 0 {
                            multiplier = i128::from(value);
                        }
                    }
                    _ => {}
                }
            }
            let Some(id) = id else { continue };
            self.multipliers.insert(id, multiplier);
            clocks.push((id, timestamp.saturating_mul(multiplier)));
        }
        let Some(&(anchor_id, anchor_ns)) = clocks.first() else {
            return Ok(());
        };
        if self.root_clock.is_none() {
            self.root_clock = Some(anchor_id);
        }
        for (id, ns) in clocks.iter().skip(1) {
            let divergence = self.clocks.union(anchor_id, *id, anchor_ns - *ns);
            if divergence > 0 {
                self.notes.insert(format!(
                    "clock snapshot disagrees with an earlier calibration by {} ns",
                    divergence
                ));
            }
        }
        Ok(())
    }

    fn parse_track(&mut self, data: &[u8]) -> Result<()> {
        let mut uuid = None;
        let mut name = None;
        let mut process_id = None;
        let mut thread_id = None;
        let mut is_counter = false;
        let mut counter_incremental = false;
        let mut unit = None;
        let mut is_state = false;
        let mut fields = Fields::new(data);
        while let Some((number, wire)) = fields.next_field()? {
            match number {
                1 => uuid = Some(wire.varint()?),
                2 | 10 => {
                    if let Ok(text) = std::str::from_utf8(wire.bytes()?) {
                        if !text.is_empty() {
                            name = Some(text.to_string());
                        }
                    }
                }
                3 => {
                    if let Ok(pid) = field_u64(wire.bytes()?, 1) {
                        process_id = Some(pid.to_string());
                    }
                }
                4 => {
                    let thread = wire.bytes()?;
                    if let Ok(pid) = field_u64(thread, 1) {
                        process_id = Some(pid.to_string());
                    }
                    if let Ok(tid) = field_u64(thread, 2) {
                        thread_id = Some(tid.to_string());
                    }
                }
                8 => {
                    is_counter = true;
                    let counter = wire.bytes()?;
                    if let Ok(text) = field_string(counter, 6) {
                        if !text.is_empty() {
                            unit = Some(text);
                        }
                    }
                    if field_u64(counter, 5).unwrap_or(0) == 1 {
                        counter_incremental = true;
                    }
                }
                18 => {
                    is_state = true;
                }
                _ => {}
            }
        }
        let Some(uuid) = uuid else { return Ok(()) };
        let entry = self.tracks.entry(uuid).or_default();
        if name.is_some() {
            entry.name = name;
        }
        if process_id.is_some() {
            entry.process_id = process_id;
        }
        if thread_id.is_some() {
            entry.thread_id = thread_id;
        }
        entry.is_counter |= is_counter;
        entry.counter_incremental |= counter_incremental;
        entry.is_state |= is_state;
        if unit.is_some() {
            entry.unit = unit;
        }
        Ok(())
    }

    fn parse_event(
        &mut self,
        data: &[u8],
        sequence_id: u64,
        packet_clock: Option<u32>,
        locator: &TraceLocator,
    ) -> Result<()> {
        let mut event_type = 0u64;
        let mut track_uuid = None;
        let mut name = None;
        let mut name_iid = None;
        let mut category = None;
        let mut category_iids = Vec::new();
        let mut counter_value = None;
        let mut double_value = None;
        let mut delta_us = None;
        let mut absolute_us = None;
        let mut annotations: Vec<&[u8]> = Vec::new();
        let mut source = None;
        let mut fields = Fields::new(data);
        while let Some((number, wire)) = fields.next_field()? {
            match number {
                9 => event_type = wire.varint()?,
                11 => track_uuid = Some(wire.varint()?),
                10 => name_iid = Some(wire.varint()?),
                23 => {
                    if let Ok(text) = std::str::from_utf8(wire.bytes()?) {
                        if !text.is_empty() {
                            name = Some(text.to_string());
                        }
                    }
                }
                22 => {
                    if let Ok(text) = std::str::from_utf8(wire.bytes()?) {
                        if !text.is_empty() {
                            category = Some(text.to_string());
                        }
                    }
                }
                3 => {
                    category_iids.push(wire.varint()?);
                }
                30 => counter_value = Some(wire.varint()? as i64),
                44 => double_value = Some(f64::from_bits(wire.fixed64()?)),
                1 => delta_us = Some(wire.varint()? as i64),
                16 => absolute_us = Some(wire.varint()? as i64),
                4 => annotations.push(wire.bytes()?),
                33 => source = Some(wire.bytes()?),
                _ => {}
            }
        }
        self.events += 1;
        if self.events > MAX_EVENTS {
            self.truncated = true;
            self.notes.insert(format!(
                "trace truncated after {MAX_EVENTS} track events; later events were ignored"
            ));
            return Ok(());
        }
        let (default_track, clock_hint) = {
            let sequence = self.sequences.entry(sequence_id).or_default();
            (sequence.default_track, sequence.clock_id)
        };
        let track_uuid = track_uuid.or(default_track).unwrap_or(0);
        let track = self.tracks.get(&track_uuid).cloned().unwrap_or_default();
        if name.is_none() {
            if let Some(iid) = name_iid {
                name = self
                    .sequences
                    .get(&sequence_id)
                    .and_then(|sequence| sequence.names.get(&iid))
                    .cloned();
            }
        }
        if category.is_none() {
            for iid in &category_iids {
                if let Some(value) = self
                    .sequences
                    .get(&sequence_id)
                    .and_then(|sequence| sequence.categories.get(iid))
                {
                    category = Some(value.clone());
                    break;
                }
            }
        }
        let mapping = {
            let empty = HashMap::new();
            let names = self
                .sequences
                .get(&sequence_id)
                .map(|sequence| &sequence.debug_names)
                .unwrap_or(&empty);
            parse_annotations(&annotations, names)?
        };
        let clock_id = packet_clock.or(clock_hint).unwrap_or(self.root_clock.unwrap_or(0));
        let mut timestamp_ns = None;
        {
            let sequence = self.sequences.entry(sequence_id).or_default();
            if let Some(absolute) = absolute_us {
                sequence.last_event_us = i128::from(absolute);
                timestamp_ns = Some(i128::from(absolute) * 1000);
            } else if let Some(delta) = delta_us {
                sequence.last_event_us = sequence
                    .last_event_us
                    .checked_add(i128::from(delta))
                    .ok_or_else(|| CoreError::invalid("track event timestamp overflow"))?;
                timestamp_ns = Some(sequence.last_event_us * 1000);
            }
        }
        let timestamp_ns = match timestamp_ns {
            Some(value) => self.convert(value, clock_id)?,
            None => {
                let ticks = self
                    .sequences
                    .get(&sequence_id)
                    .map(|sequence| sequence.timestamp_accum)
                    .unwrap_or(0);
                let value = self.convert(ticks, clock_id)?;
                if let Some(sequence) = self.sequences.get_mut(&sequence_id) {
                    sequence.last_event_us = value.div_euclid(1000);
                }
                value
            }
        };
        match event_type {
            SLICE_BEGIN => {
                let name = name
                    .or_else(|| track.name.clone())
                    .unwrap_or_else(|| "(unnamed slice)".to_string());
                let mut attributes = Map::new();
                if let Some(category) = &category {
                    attributes.insert("capture.category".to_string(), json!(category));
                }
                attributes.insert("perfetto.track_uuid".to_string(), json!(track_uuid.to_string()));
                attributes.insert("perfetto.packet_sequence".to_string(), json!(sequence_id.to_string()));
                add_mapping_attributes(&mapping, &mut attributes);
                attributes.extend(mapping.extra.clone());
                let entity_id = mapping
                    .entity_id
                    .clone()
                    .or_else(|| source_entity(source, locator, &mut attributes));
                self.open_slices.entry(track_uuid).or_default().push(OpenSlice {
                    start_ns: timestamp_ns,
                    name,
                    category,
                    entity_id,
                    attributes,
                    process_id: track.process_id.clone(),
                    thread_id: track.thread_id.clone(),
                });
            }
            SLICE_END => {
                let Some(mut open) = self
                    .open_slices
                    .get_mut(&track_uuid)
                    .and_then(Vec::pop)
                else {
                    self.unclosed_slices += 1;
                    return Ok(());
                };
                if let Some(category) = &open.category {
                    open.attributes
                        .entry("capture.category".to_string())
                        .or_insert_with(|| json!(category));
                }
                self.spans.push(SpanDraft {
                    id: format!("perfetto.span.{}", self.spans.len() + 1),
                    parent: None,
                    name: open.name,
                    kind: "slice".to_string(),
                    start_ns: open.start_ns,
                    end_ns: timestamp_ns.max(open.start_ns),
                    process_id: open.process_id,
                    thread_id: open.thread_id,
                    entity_id: open.entity_id,
                    attributes: open.attributes,
                });
            }
            INSTANT => {
                let name = name
                    .or_else(|| track.name.clone())
                    .unwrap_or_else(|| "(unnamed instant)".to_string());
                let mut attributes = Map::new();
                if let Some(category) = &category {
                    attributes.insert("capture.category".to_string(), json!(category));
                }
                attributes.insert("perfetto.track_uuid".to_string(), json!(track_uuid.to_string()));
                attributes.insert("perfetto.phase".to_string(), json!("instant"));
                add_mapping_attributes(&mapping, &mut attributes);
                attributes.extend(mapping.extra.clone());
                let entity_id = mapping
                    .entity_id
                    .clone()
                    .or_else(|| source_entity(source, locator, &mut attributes));
                self.spans.push(SpanDraft {
                    id: format!("perfetto.span.{}", self.spans.len() + 1),
                    parent: None,
                    name,
                    kind: "instant".to_string(),
                    start_ns: timestamp_ns,
                    end_ns: timestamp_ns,
                    process_id: track.process_id.clone(),
                    thread_id: track.thread_id.clone(),
                    entity_id,
                    attributes,
                });
            }
            COUNTER => {
                let value = match (counter_value, double_value) {
                    (Some(value), _) => json!(value),
                    (None, Some(value)) if value.is_finite() => json!(value),
                    (None, Some(_)) => {
                        self.notes
                            .insert("a non-finite counter value was dropped".to_string());
                        return Ok(());
                    }
                    (None, None) => {
                        self.notes.insert(format!(
                            "counter event on track {track_uuid} has no numeric value"
                        ));
                        return Ok(());
                    }
                };
                let value = if track.counter_incremental {
                    let running = self.incremental_counters.entry(track_uuid).or_insert(0);
                    let delta = i128::from(value.as_i64().unwrap_or(0));
                    *running = running.saturating_add(delta);
                    json!(*running)
                } else {
                    value
                };
                let series = self.counters.entry(track_uuid).or_insert_with(|| {
                    let mut attributes = Map::new();
                    attributes.insert(
                        "perfetto.track_uuid".to_string(),
                        json!(track_uuid.to_string()),
                    );
                    if track.counter_incremental {
                        attributes.insert("perfetto.incremental".to_string(), json!(true));
                    }
                    CounterSeries {
                        name: track
                            .name
                            .clone()
                            .unwrap_or_else(|| format!("counter.{track_uuid}")),
                        unit: track.unit.clone(),
                        entity_id: None,
                        attributes,
                        samples: Vec::new(),
                    }
                });
                if series.entity_id.is_none() {
                    series.entity_id = mapping.entity_id.clone();
                }
                series.samples.push(json!({
                    "timestamp_ns": timestamp_ns.to_string(),
                    "value": value,
                    "process_id": track.process_id,
                    "thread_id": track.thread_id,
                }));
            }
            STATE => {
                let value = name.unwrap_or_else(|| "(unnamed state)".to_string());
                let series = self.states.entry(track_uuid).or_insert_with(|| {
                    let mut attributes = Map::new();
                    attributes.insert(
                        "perfetto.track_uuid".to_string(),
                        json!(track_uuid.to_string()),
                    );
                    StateSeries {
                        name: track
                            .name
                            .clone()
                            .unwrap_or_else(|| format!("state.{track_uuid}")),
                        entity_id: None,
                        attributes,
                        intervals: Vec::new(),
                    }
                });
                if series.entity_id.is_none() {
                    series.entity_id = mapping.entity_id.clone();
                }
                if let Some(previous) = self.open_states.remove(&track_uuid) {
                    series.intervals.push(json!({
                        "start_ns": previous.start_ns.to_string(),
                        "end_ns": timestamp_ns.max(previous.start_ns).to_string(),
                        "value": previous.value,
                        "process_id": track.process_id,
                        "thread_id": track.thread_id,
                    }));
                }
                self.open_states.insert(
                    track_uuid,
                    OpenState {
                        start_ns: timestamp_ns,
                        value: json!(value),
                    },
                );
            }
            other => {
                self.notes.insert(format!(
                    "unsupported track event type {other} was skipped"
                ));
            }
        }
        Ok(())
    }

    fn convert(&mut self, ticks: i128, clock_id: u32) -> Result<i128> {
        let multiplier = self.multipliers.get(&clock_id).copied().unwrap_or(1);
        if self.root_clock.is_none() {
            self.root_clock = Some(clock_id);
        }
        if !self.clocks.contains(clock_id) && self.unknown_clocks.insert(clock_id) {
            self.notes.insert(format!(
                "clock {clock_id} has no clock snapshot; it is used as its own domain"
            ));
        }
        let (_, offset) = self.clocks.resolve(clock_id);
        ticks
            .checked_mul(multiplier)
            .and_then(|value| value.checked_add(offset))
            .ok_or_else(|| CoreError::invalid("perfetto timestamp overflow"))
    }

    fn finish(mut self, meta: &TraceMeta) -> Result<Value> {
        if self.unclosed_slices > 0 {
            self.notes.insert(format!(
                "{} slice end events had no matching begin and were ignored",
                self.unclosed_slices
            ));
        }
        for (track_uuid, open) in &self.open_slices {
            if open.is_empty() {
                continue;
            }
            self.notes.insert(format!(
                "{} open slices on track {track_uuid} had no end event and were dropped",
                open.len()
            ));
        }
        if !self.open_states.is_empty() {
            self.notes.insert(format!(
                "{} state tracks have an open final interval at trace end; it is not closed with a fabricated end",
                self.open_states.len()
            ));
        }
        let counters = self
            .counters
            .into_values()
            .map(|series| {
                json!({
                    "counter_id": format!("perfetto.counter.{}", sanitize(&series.name)),
                    "name": series.name,
                    "unit": series.unit,
                    "clock_id": "default",
                    "entity_id": series.entity_id,
                    "attributes": series.attributes,
                    "samples": series.samples,
                })
            })
            .collect::<Vec<_>>();
        let states = self
            .states
            .into_values()
            .map(|series| {
                json!({
                    "state_id": format!("perfetto.state.{}", sanitize(&series.name)),
                    "name": series.name,
                    "clock_id": "default",
                    "entity_id": series.entity_id,
                    "attributes": series.attributes,
                    "intervals": series.intervals,
                })
            })
            .collect::<Vec<_>>();
        let mut meta = meta.clone();
        meta.clock_kind = match self.root_clock {
            Some(6) => "perfetto_boottime_ns".to_string(),
            Some(clock) => format!("perfetto_clock_{clock}_ns"),
            None => "perfetto_trace_clock_ns".to_string(),
        };
        if !self.notes.is_empty() {
            meta.lossy = true;
            meta.loss_notes.extend(self.notes);
        }
        base_trace_with_metrics(&meta, self.spans, counters, states)
    }
}

fn sanitize(name: &str) -> String {
    let mut value: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | ':') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if value.len() > 120 {
        value.truncate(120);
    }
    if value.is_empty() {
        value.push_str("metric");
    }
    value
}

#[derive(Default)]
struct Annotations {
    entity_id: Option<String>,
    probe_id: Option<String>,
    extra: Map<String, Value>,
}

fn parse_annotations(values: &[&[u8]], names: &HashMap<u64, String>) -> Result<Annotations> {
    let mut annotations = Annotations::default();
    for raw in values {
        let mut name = None;
        let mut name_iid = None;
        let mut value: Option<Value> = None;
        let mut fields = Fields::new(raw);
        while let Some((number, wire)) = fields.next_field()? {
            match number {
                10 => {
                    if let Ok(text) = std::str::from_utf8(wire.bytes()?) {
                        name = Some(text.to_string());
                    }
                }
                1 => name_iid = Some(wire.varint()?),
                2 => value = Some(json!(wire.varint()? != 0)),
                3 => value = Some(json!(wire.varint()?)),
                4 => value = Some(json!(wire.varint()? as i64)),
                5 => value = Some(json!(f64::from_bits(wire.fixed64()?))),
                6 => {
                    if let Ok(text) = std::str::from_utf8(wire.bytes()?) {
                        value = Some(json!(text));
                    }
                }
                _ => {}
            }
        }
        let Some(name) = name.or_else(|| name_iid.and_then(|iid| names.get(&iid).cloned())) else {
            continue;
        };
        let Some(value) = value else { continue };
        match name.as_str() {
            "archmeld.entity_id" => {
                annotations.entity_id = value.as_str().map(str::to_string);
            }
            "archmeld.probe_id" => {
                annotations.probe_id = value.as_str().map(str::to_string);
            }
            _ => {
                annotations
                    .extra
                    .insert(format!("perfetto.debug.{name}"), value);
            }
        }
    }
    Ok(annotations)
}

fn add_mapping_attributes(annotations: &Annotations, attributes: &mut Map<String, Value>) {
    if let Some(probe) = &annotations.probe_id {
        attributes.insert("archmeld.probe_id".to_string(), json!(probe));
    }
}

fn source_entity(
    raw: Option<&[u8]>,
    locator: &TraceLocator,
    attributes: &mut Map<String, Value>,
) -> Option<String> {
    let raw = raw?;
    let file = field_string(raw, 2).ok()?;
    let function = field_string(raw, 3).ok();
    let line = field_u64(raw, 4).ok();
    attributes.insert("perfetto.source.file".to_string(), json!(file));
    if let Some(function) = &function {
        attributes.insert("perfetto.source.function".to_string(), json!(function));
    }
    if let Some(line) = line {
        attributes.insert("perfetto.source.line".to_string(), json!(line));
        return locator.location(&file, line);
    }
    None
}

fn parse_defaults(data: &[u8], sequence: &mut SequenceState) -> Result<()> {
    let mut fields = Fields::new(data);
    while let Some((number, wire)) = fields.next_field()? {
        match number {
            58 => sequence.clock_id = Some(wire.varint()? as u32),
            11 => {
                let defaults = wire.bytes()?;
                let mut inner = Fields::new(defaults);
                while let Some((number, wire)) = inner.next_field()? {
                    if number == 11 {
                        sequence.default_track = Some(wire.varint()?);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_interned(data: &[u8], sequence: &mut SequenceState) -> Result<()> {
    let mut fields = Fields::new(data);
    while let Some((number, wire)) = fields.next_field()? {
        let target = match number {
            1 => &mut sequence.categories,
            2 => &mut sequence.names,
            3 => &mut sequence.debug_names,
            _ => continue,
        };
        let entry = wire.bytes()?;
        let mut iid = None;
        let mut name = None;
        let mut inner = Fields::new(entry);
        while let Some((number, wire)) = inner.next_field()? {
            match number {
                1 => iid = Some(wire.varint()?),
                2 => {
                    if let Ok(text) = std::str::from_utf8(wire.bytes()?) {
                        name = Some(text.to_string());
                    }
                }
                _ => {}
            }
        }
        if let (Some(iid), Some(name)) = (iid, name) {
            target.insert(iid, name);
        }
    }
    Ok(())
}

fn field_u64(data: &[u8], target: u32) -> Result<u64> {
    let mut fields = Fields::new(data);
    while let Some((number, wire)) = fields.next_field()? {
        if number == target {
            return wire.varint();
        }
    }
    Err(CoreError::invalid(format!("field {target} missing")))
}

fn field_string(data: &[u8], target: u32) -> Result<String> {
    let mut fields = Fields::new(data);
    while let Some((number, wire)) = fields.next_field()? {
        if number == target {
            return std::str::from_utf8(wire.bytes()?)
                .map(str::to_string)
                .map_err(|_| CoreError::invalid("invalid utf-8 in perfetto protobuf"));
        }
    }
    Err(CoreError::invalid(format!("field {target} missing")))
}

/// Union-find over clock ids with a relative offset. `offset[c]` is added to a
/// timestamp in `c` to express it in the parent's timebase.
#[derive(Default)]
struct Clocks {
    parent: HashMap<u32, u32>,
    offset: HashMap<u32, i128>,
}

impl Clocks {
    fn contains(&self, id: u32) -> bool {
        self.parent.contains_key(&id)
    }

    fn resolve(&self, id: u32) -> (u32, i128) {
        let mut current = id;
        let mut offset = 0i128;
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 256 {
                return (current, offset);
            }
            match self.parent.get(&current) {
                Some(parent) => {
                    offset += self.offset.get(&current).copied().unwrap_or(0);
                    current = *parent;
                }
                None => return (current, offset),
            }
        }
    }

    /// `timestamp_a = timestamp_b + delta`. Returns calibration divergence in
    /// ns when both clocks were already connected.
    fn union(&mut self, a: u32, b: u32, delta: i128) -> i128 {
        let (root_a, offset_a) = self.resolve(a);
        let (root_b, offset_b) = self.resolve(b);
        if root_a == root_b {
            let measured = offset_b - offset_a;
            return (measured - delta).abs();
        }
        self.parent.insert(root_b, root_a);
        self.offset.insert(root_b, delta + offset_a - offset_b);
        0
    }
}

enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    #[allow(dead_code)]
    Fixed32(u32),
    Bytes(&'a [u8]),
}

impl<'a> Wire<'a> {
    fn varint(self) -> Result<u64> {
        match self {
            Wire::Varint(value) => Ok(value),
            _ => Err(CoreError::invalid("expected a protobuf varint")),
        }
    }

    fn fixed64(self) -> Result<u64> {
        match self {
            Wire::Fixed64(value) => Ok(value),
            _ => Err(CoreError::invalid("expected a protobuf fixed64")),
        }
    }

    fn bytes(self) -> Result<&'a [u8]> {
        match self {
            Wire::Bytes(value) => Ok(value),
            _ => Err(CoreError::invalid("expected protobuf bytes")),
        }
    }
}

struct Fields<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Fields<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn next_field(&mut self) -> Result<Option<(u32, Wire<'a>)>> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        let key = read_varint(self.data, &mut self.pos)?;
        let number = (key >> 3) as u32;
        if number == 0 {
            return Err(CoreError::invalid("invalid protobuf field number 0"));
        }
        let wire = match key & 7 {
            0 => Wire::Varint(read_varint(self.data, &mut self.pos)?),
            1 => {
                let value = read_fixed64(self.data, &mut self.pos)?;
                Wire::Fixed64(value)
            }
            2 => {
                let length = read_varint(self.data, &mut self.pos)? as usize;
                if length > self.data.len().saturating_sub(self.pos) {
                    return Err(CoreError::invalid("protobuf length exceeds the buffer"));
                }
                let start = self.pos;
                self.pos += length;
                Wire::Bytes(&self.data[start..start + length])
            }
            5 => {
                let value = read_fixed32(self.data, &mut self.pos)?;
                Wire::Fixed32(value)
            }
            other => {
                return Err(CoreError::unsupported(format!(
                    "protobuf wire type {other} is not supported"
                )))
            }
        };
        Ok(Some((number, wire)))
    }
}

fn read_varint(data: &[u8], pos: &mut usize) -> Result<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        if *pos >= data.len() {
            return Err(CoreError::invalid("truncated protobuf varint"));
        }
        let byte = data[*pos];
        *pos += 1;
        if shift == 63 && byte > 1 {
            return Err(CoreError::invalid("protobuf varint overflows u64"));
        }
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err(CoreError::invalid("protobuf varint is too long"));
        }
    }
}

fn read_fixed64(data: &[u8], pos: &mut usize) -> Result<u64> {
    if data.len().saturating_sub(*pos) < 8 {
        return Err(CoreError::invalid("truncated protobuf fixed64"));
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[*pos..*pos + 8]);
    *pos += 8;
    Ok(u64::from_le_bytes(bytes))
}

fn read_fixed32(data: &[u8], pos: &mut usize) -> Result<u32> {
    if data.len().saturating_sub(*pos) < 4 {
        return Err(CoreError::invalid("truncated protobuf fixed32"));
    }
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&data[*pos..*pos + 4]);
    *pos += 4;
    Ok(u32::from_le_bytes(bytes))
}
