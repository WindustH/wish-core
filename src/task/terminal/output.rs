//! A bounded PTY history. Live WebSocket readers apply backpressure before their
//! unread bytes are evicted. A reader which lost history resumes at a VT boundary.
use std::{
  collections::{HashMap, VecDeque},
  sync::{Arc, Condvar, Mutex},
  time::{Duration, Instant},
};

use super::{RETAINED, lock};

#[derive(Clone, Copy, Default, PartialEq)]
enum Boundary {
  #[default]
  Text,
  Escape,
  EscapeIntermediate,
  Csi,
  String {
    osc: bool,
  },
  StringEscape {
    osc: bool,
  },
}

impl Boundary {
  fn after(self, byte: u8) -> Self {
    use Boundary::*;
    match (self, byte) {
      (_, 0x18 | 0x1a) => Text, // CAN, SUB
      (StringEscape { .. }, b'\\') | (String { osc: true } | StringEscape { osc: true }, 7) => Text,
      (String { osc } | StringEscape { osc }, 0x1b) => StringEscape { osc },
      (String { osc } | StringEscape { osc }, _) => String { osc },
      (_, 0x1b) => Escape,
      (Escape, b']') => String { osc: true },
      (Escape, b'P' | b'_' | b'^' | b'X') => String { osc: false },
      (Escape, b'[') => Csi,
      (Escape | EscapeIntermediate, 0x20..=0x2f) => EscapeIntermediate,
      (Escape | EscapeIntermediate, 0x30..=0x7e) | (Csi, 0x40..=0x7e) => Text,
      _ => self,
    }
  }
}

#[derive(Default)]
pub(super) struct Buffer {
  bytes: VecDeque<u8>,
  pub end: u64,
  pub eof: bool,
  closing: bool,
  head: Boundary,
  tail: Boundary,
  last_boundary: u64,
  readers: HashMap<u64, u64>,
  serial: u64,
  first_reader_until: Option<Instant>,
}

impl Buffer {
  pub(super) fn start(&self) -> u64 {
    self.end - self.bytes.len() as u64
  }
  fn append(&mut self, bytes: &[u8]) {
    let remove = (self.bytes.len() + bytes.len()).saturating_sub(RETAINED);
    for byte in self.bytes.drain(..remove) {
      self.head = self.head.after(byte);
    }
    for (index, &byte) in bytes.iter().enumerate() {
      self.tail = self.tail.after(byte);
      if self.tail == Boundary::Text {
        self.last_boundary = self.end + index as u64 + 1;
      }
    }
    self.end += bytes.len() as u64;
    self.bytes.extend(bytes);
  }
  pub fn read(&self, offset: u64) -> Option<(bool, u64, Vec<u8>)> {
    let start = self.start();
    let reset = offset < start || offset > self.end;
    let index = if reset {
      if self.last_boundary < start && !self.eof {
        return None;
      }
      let mut state = self.head;
      self
        .bytes
        .iter()
        .position(|&byte| {
          let safe = state == Boundary::Text && byte & 0xc0 != 0x80;
          state = state.after(byte);
          safe
        })
        .unwrap_or(self.bytes.len())
    } else {
      (offset - start) as usize
    };
    if index == self.bytes.len() && !self.eof && (!reset || self.tail != Boundary::Text) {
      return None;
    }
    let bytes: Vec<_> = self.bytes.range(index..).take(65536).copied().collect();
    Some((reset, start + (index + bytes.len()) as u64, bytes))
  }
}

pub(super) struct Output {
  pub buffer: Mutex<Buffer>,
  space: Condvar,
}

impl Output {
  pub fn new() -> Self {
    // A login profile can emit an entire image between start and WS upgrade.
    let buffer = Buffer {
      first_reader_until: Some(Instant::now() + Duration::from_secs(5)),
      ..Buffer::default()
    };
    Self { buffer: Mutex::new(buffer), space: Condvar::new() }
  }
  pub fn release_startup(&self) {
    if lock(&self.buffer).first_reader_until.take().is_some() {
      self.space.notify_one();
    }
  }
  pub fn push(&self, bytes: &[u8]) {
    let mut buffer = lock(&self.buffer);
    while !buffer.closing {
      let start = buffer.start();
      let evict_to = (buffer.end + bytes.len() as u64).saturating_sub(RETAINED as u64);
      if evict_to > 0
        && let Some(deadline) = buffer.first_reader_until
        && let Some(wait) = deadline.checked_duration_since(Instant::now())
      {
        buffer = self.space.wait_timeout(buffer, wait).unwrap_or_else(|error| error.into_inner()).0;
        continue;
      }
      // Already expired cursors cannot recover the lost prefix. Let them catch
      // up at a complete sequence while the producer continues.
      if !buffer.readers.values().any(|&pos| pos >= start && pos < evict_to) {
        break;
      }
      buffer = self.space.wait(buffer).unwrap_or_else(|error| error.into_inner());
    }
    buffer.append(bytes);
  }
  pub fn close(&self) {
    lock(&self.buffer).closing = true;
    self.space.notify_all();
  }
  pub fn subscribe(self: &Arc<Self>, offset: u64) -> OutputReader {
    let mut buffer = lock(&self.buffer);
    buffer.serial += 1;
    let id = buffer.serial;
    buffer.readers.insert(id, offset);
    buffer.first_reader_until = None;
    self.space.notify_one();
    OutputReader { output: self.clone(), id }
  }
}

/// Dropping a parked or disconnected socket always unblocks the PTY pump.
pub(crate) struct OutputReader {
  output: Arc<Output>,
  id: u64,
}
impl OutputReader {
  pub fn advance(&self, offset: u64) {
    lock(&self.output.buffer).readers.insert(self.id, offset);
    self.output.space.notify_one();
  }
}
impl Drop for OutputReader {
  fn drop(&mut self) {
    lock(&self.output.buffer).readers.remove(&self.id);
    self.output.space.notify_one();
  }
}
