//! Business-side priority packing and incremental message reconstruction.
//!
//! A batch contains lane/length/byte segments. Segments may split anywhere in a
//! serialized message, including its length prefix. Independent lane assemblers
//! let a newly queued game message precede a resource message's remaining bytes
//! without corrupting either FIFO. The byte transport never reads this format.

use crate::{NetworkError, transport::MAX_BATCH_BYTES};
use roundo_contracts::{ClientMessage, ServerMessage, TrafficClass};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::VecDeque, marker::PhantomData};
use tokio::sync::mpsc;

const LENGTH_BYTES: usize = size_of::<u32>();
const SEGMENT_HEADER: usize = size_of::<u8>() + LENGTH_BYTES;

pub(crate) trait Routed: Serialize + DeserializeOwned {
    fn class(&self) -> Option<TrafficClass>;
}
impl Routed for ClientMessage {
    fn class(&self) -> Option<TrafficClass> {
        self.traffic_class()
    }
}
impl Routed for ServerMessage {
    fn class(&self) -> Option<TrafficClass> {
        self.traffic_class()
    }
}

struct Record {
    bytes: Vec<u8>,
    offset: usize,
}
pub(crate) struct BatchWriter<M> {
    queues: [mpsc::Receiver<M>; 2],
    partial: [Option<Record>; 2],
    control: VecDeque<M>,
    application_open: bool,
}
impl<M: Routed> BatchWriter<M> {
    pub(crate) fn new(game: mpsc::Receiver<M>, resource: mpsc::Receiver<M>) -> Self {
        Self {
            queues: [game, resource],
            partial: [None, None],
            control: VecDeque::new(),
            application_open: false,
        }
    }
    pub(crate) fn control(&mut self, message: M) {
        self.control.push_back(message);
    }
    pub(crate) fn open(&mut self) {
        self.application_open = true;
    }

    /// Session boundaries revoke all encoded fragments and queued business work.
    /// No old message or partial record may migrate to a fresh byte stream.
    pub(crate) fn reset(&mut self) {
        self.application_open = false;
        self.control.clear();
        self.partial = [None, None];
        for queue in &mut self.queues {
            while queue.try_recv().is_ok() {}
        }
    }
    pub(crate) fn idle(&self) -> bool {
        self.control.is_empty() && self.partial.iter().all(Option::is_none)
    }

    pub(crate) fn prepare(&mut self) -> Result<Option<Vec<u8>>, NetworkError> {
        let mut batch = Vec::new();
        for class in TrafficClass::ALL {
            let index = class as usize;
            while MAX_BATCH_BYTES - batch.len() > SEGMENT_HEADER {
                if self.partial[index].is_none() {
                    let message = if class == TrafficClass::Game {
                        self.control.pop_front().or_else(|| {
                            self.application_open
                                .then(|| self.queues[index].try_recv().ok())
                                .flatten()
                        })
                    } else if self.application_open {
                        self.queues[index].try_recv().ok()
                    } else {
                        None
                    };
                    let Some(message) = message else {
                        break;
                    };
                    let payload =
                        postcard::to_allocvec(&message).map_err(NetworkError::from_display)?;
                    let length = u32::try_from(payload.len()).map_err(|_| {
                        NetworkError::new("business message exceeds wire length representation")
                    })?;
                    let mut bytes = Vec::with_capacity(LENGTH_BYTES + payload.len());
                    bytes.extend_from_slice(&length.to_be_bytes());
                    bytes.extend_from_slice(&payload);
                    self.partial[index] = Some(Record { bytes, offset: 0 });
                }
                let record = self.partial[index].as_mut().expect("prepared record");
                let count = (MAX_BATCH_BYTES - batch.len() - SEGMENT_HEADER)
                    .min(record.bytes.len() - record.offset);
                batch.push(class as u8);
                batch.extend_from_slice(&(count as u32).to_be_bytes());
                batch.extend_from_slice(&record.bytes[record.offset..record.offset + count]);
                record.offset += count;
                if record.offset == record.bytes.len() {
                    self.partial[index] = None;
                }
            }
        }
        Ok((!batch.is_empty()).then_some(batch))
    }
}

#[derive(Default)]
struct Assembly {
    bytes: Vec<u8>,
    length: Option<usize>,
}
impl Assembly {
    fn feed<M: DeserializeOwned>(
        &mut self,
        mut input: &[u8],
        output: &mut Vec<M>,
    ) -> Result<(), NetworkError> {
        while !input.is_empty() {
            let target = self.length.unwrap_or(LENGTH_BYTES);
            let count = (target - self.bytes.len()).min(input.len());
            // Grow only for bytes actually received, never for a peer's declared
            // message length. Producer and consumer admission remain business policy.
            self.bytes
                .try_reserve(count)
                .map_err(NetworkError::from_display)?;
            self.bytes.extend_from_slice(&input[..count]);
            input = &input[count..];
            if self.bytes.len() != target {
                continue;
            }
            if self.length.is_none() {
                let length = u32::from_be_bytes(self.bytes[..].try_into().expect("complete length"))
                    as usize;
                if length == 0 {
                    return Err(NetworkError::new("empty serialized business message"));
                }
                self.bytes.clear();
                self.length = Some(length);
            } else {
                let (message, rest) = postcard::take_from_bytes::<M>(&self.bytes)
                    .map_err(NetworkError::from_display)?;
                if !rest.is_empty() {
                    return Err(NetworkError::new("trailing bytes in business message"));
                }
                output.push(message);
                self.bytes.clear();
                self.length = None;
            }
        }
        Ok(())
    }
}

pub(crate) struct BatchReader<M> {
    lanes: [Assembly; 2],
    marker: PhantomData<M>,
}
impl<M: Routed> Default for BatchReader<M> {
    fn default() -> Self {
        Self {
            lanes: Default::default(),
            marker: PhantomData,
        }
    }
}
impl<M: Routed> BatchReader<M> {
    pub(crate) fn receive(&mut self, mut batch: &[u8]) -> Result<Vec<M>, NetworkError> {
        let mut output = Vec::<M>::new();
        while !batch.is_empty() {
            if batch.len() < SEGMENT_HEADER {
                return Err(NetworkError::new("incomplete application batch segment"));
            }
            let class = TrafficClass::from_wire(batch[0])
                .ok_or_else(|| NetworkError::new("unknown business lane"))?;
            let length = u32::from_be_bytes(
                batch[1..SEGMENT_HEADER]
                    .try_into()
                    .expect("checked segment header"),
            ) as usize;
            batch = &batch[SEGMENT_HEADER..];
            if length == 0 || length > batch.len() {
                return Err(NetworkError::new(
                    "invalid application batch segment length",
                ));
            }
            let start = output.len();
            self.lanes[class as usize].feed(&batch[..length], &mut output)?;
            if output[start..]
                .iter()
                .any(|message| message.class().unwrap_or(TrafficClass::Game) != class)
            {
                return Err(NetworkError::new(
                    "business message on the wrong priority lane",
                ));
            }
            batch = &batch[length..];
        }
        Ok(output)
    }
}
