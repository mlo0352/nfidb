use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROTOCOL_VERSION: u8 = 1;
const MESSAGE_POINTER_BATCH: u8 = 1;
const BATCH_HEADER_LEN: usize = 16;
const SAMPLE_LEN: usize = 44;
const MAX_SAMPLES_PER_BATCH: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum DeviceType {
    Pen = 1,
    Touch = 2,
}

impl TryFrom<u8> for DeviceType {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Pen),
            2 => Ok(Self::Touch),
            other => Err(ProtocolError::InvalidDevice(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Action {
    Down = 1,
    Move = 2,
    Up = 3,
    Cancel = 4,
    Hover = 5,
}

impl Action {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Up | Self::Cancel)
    }

    #[must_use]
    pub const fn is_contact(self) -> bool {
        matches!(self, Self::Down | Self::Move)
    }
}

impl TryFrom<u8> for Action {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Down),
            2 => Ok(Self::Move),
            3 => Ok(Self::Up),
            4 => Ok(Self::Cancel),
            5 => Ok(Self::Hover),
            other => Err(ProtocolError::InvalidAction(other)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PointerSample {
    pub device_type: DeviceType,
    pub action: Action,
    pub flags: u16,
    pub pointer_id: u32,
    pub sample_sequence: u32,
    pub x_norm: f32,
    pub y_norm: f32,
    pub pressure: f32,
    pub tilt_x_deg: f32,
    pub tilt_y_deg: f32,
    pub twist_deg: f32,
    pub client_time_ms: f64,
}

impl fmt::Debug for PointerSample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointerSample")
            .field("device_type", &self.device_type)
            .field("action", &self.action)
            .field("pointer_id", &self.pointer_id)
            .field("sample_sequence", &self.sample_sequence)
            .field("position", &(self.x_norm, self.y_norm))
            .field("pressure", &self.pressure)
            .field("tilt", &(self.tilt_x_deg, self.tilt_y_deg))
            .finish_non_exhaustive()
    }
}

impl PointerSample {
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.x_norm = finite_or(self.x_norm, 0.0).clamp(0.0, 1.0);
        self.y_norm = finite_or(self.y_norm, 0.0).clamp(0.0, 1.0);
        self.pressure = finite_or(self.pressure, 0.0).clamp(0.0, 1.0);
        self.tilt_x_deg = finite_or(self.tilt_x_deg, 0.0).clamp(-90.0, 90.0);
        self.tilt_y_deg = finite_or(self.tilt_y_deg, 0.0).clamp(-90.0, 90.0);
        self.twist_deg = finite_or(self.twist_deg, 0.0).rem_euclid(360.0);
        self.client_time_ms = if self.client_time_ms.is_finite() {
            self.client_time_ms
        } else {
            0.0
        };
        self
    }

    #[must_use]
    pub fn pressure_u32(self) -> u32 {
        (self.sanitized().pressure * 1024.0).round() as u32
    }

    #[must_use]
    pub fn tilt_i32(self) -> (i32, i32) {
        let sample = self.sanitized();
        (sample.tilt_x_deg.round() as i32, sample.tilt_y_deg.round() as i32)
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointerBatch {
    pub batch_sequence: u32,
    pub client_send_time_ms: f64,
    pub samples: Vec<PointerSample>,
}

impl PointerBatch {
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        BATCH_HEADER_LEN + SAMPLE_LEN * self.samples.len()
    }

    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.samples.len() > MAX_SAMPLES_PER_BATCH {
            return Err(ProtocolError::TooManySamples(self.samples.len()));
        }

        let mut bytes = Vec::with_capacity(self.encoded_len());
        bytes.push(PROTOCOL_VERSION);
        bytes.push(MESSAGE_POINTER_BATCH);
        put_u16(&mut bytes, self.samples.len() as u16);
        put_u32(&mut bytes, self.batch_sequence);
        put_f64(&mut bytes, self.client_send_time_ms);

        for sample in self.samples.iter().copied().map(PointerSample::sanitized) {
            bytes.push(sample.device_type as u8);
            bytes.push(sample.action as u8);
            put_u16(&mut bytes, sample.flags);
            put_u32(&mut bytes, sample.pointer_id);
            put_u32(&mut bytes, sample.sample_sequence);
            put_f32(&mut bytes, sample.x_norm);
            put_f32(&mut bytes, sample.y_norm);
            put_f32(&mut bytes, sample.pressure);
            put_f32(&mut bytes, sample.tilt_x_deg);
            put_f32(&mut bytes, sample.tilt_y_deg);
            put_f32(&mut bytes, sample.twist_deg);
            put_f64(&mut bytes, sample.client_time_ms);
        }

        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < BATCH_HEADER_LEN {
            return Err(ProtocolError::Truncated {
                expected: BATCH_HEADER_LEN,
                actual: bytes.len(),
            });
        }

        let mut cursor = Cursor::new(bytes);
        let version = cursor.u8()?;
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidVersion(version));
        }
        let message_type = cursor.u8()?;
        if message_type != MESSAGE_POINTER_BATCH {
            return Err(ProtocolError::InvalidMessageType(message_type));
        }
        let sample_count = usize::from(cursor.u16()?);
        if sample_count > MAX_SAMPLES_PER_BATCH {
            return Err(ProtocolError::TooManySamples(sample_count));
        }
        let expected = BATCH_HEADER_LEN + sample_count * SAMPLE_LEN;
        if bytes.len() != expected {
            return Err(if bytes.len() < expected {
                ProtocolError::Truncated {
                    expected,
                    actual: bytes.len(),
                }
            } else {
                ProtocolError::TrailingBytes(bytes.len() - expected)
            });
        }

        let batch_sequence = cursor.u32()?;
        let client_send_time_ms = cursor.f64()?;
        let mut samples = Vec::with_capacity(sample_count);
        for _ in 0..sample_count {
            samples.push(
                PointerSample {
                    device_type: DeviceType::try_from(cursor.u8()?)?,
                    action: Action::try_from(cursor.u8()?)?,
                    flags: cursor.u16()?,
                    pointer_id: cursor.u32()?,
                    sample_sequence: cursor.u32()?,
                    x_norm: cursor.f32()?,
                    y_norm: cursor.f32()?,
                    pressure: cursor.f32()?,
                    tilt_x_deg: cursor.f32()?,
                    tilt_y_deg: cursor.f32()?,
                    twist_deg: cursor.f32()?,
                    client_time_ms: cursor.f64()?,
                }
                .sanitized(),
            );
        }
        Ok(Self {
            batch_sequence,
            client_send_time_ms: if client_send_time_ms.is_finite() {
                client_send_time_ms
            } else {
                0.0
            },
            samples,
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unsupported protocol version {0}")]
    InvalidVersion(u8),
    #[error("unsupported message type {0}")]
    InvalidMessageType(u8),
    #[error("invalid pointer device type {0}")]
    InvalidDevice(u8),
    #[error("invalid pointer action {0}")]
    InvalidAction(u8),
    #[error("pointer batch contains {0} samples; maximum is {MAX_SAMPLES_PER_BATCH}")]
    TooManySamples(usize),
    #[error("truncated packet: expected {expected} bytes, received {actual}")]
    Truncated { expected: usize, actual: usize },
    #[error("packet contains {0} trailing bytes")]
    TrailingBytes(usize),
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let end = self.position.saturating_add(N);
        let slice = self.bytes.get(self.position..end).ok_or(ProtocolError::Truncated {
            expected: end,
            actual: self.bytes.len(),
        })?;
        self.position = end;
        let mut value = [0_u8; N];
        value.copy_from_slice(slice);
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn f32(&mut self) -> Result<f32, ProtocolError> {
        Ok(f32::from_le_bytes(self.take()?))
    }

    fn f64(&mut self) -> Result<f64, ProtocolError> {
        Ok(f64::from_le_bytes(self.take()?))
    }
}

fn put_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_f64(bytes: &mut Vec<u8>, value: f64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PointerSample {
        PointerSample {
            device_type: DeviceType::Pen,
            action: Action::Move,
            flags: 7,
            pointer_id: 42,
            sample_sequence: u32::MAX,
            x_norm: 0.25,
            y_norm: 0.75,
            pressure: 0.5,
            tilt_x_deg: -30.0,
            tilt_y_deg: 31.0,
            twist_deg: 123.0,
            client_time_ms: 456.25,
        }
    }

    #[test]
    fn binary_round_trip_is_exact() {
        let batch = PointerBatch {
            batch_sequence: 99,
            client_send_time_ms: 1000.5,
            samples: vec![
                sample(),
                PointerSample {
                    action: Action::Up,
                    ..sample()
                },
            ],
        };
        let encoded = batch.encode().expect("batch encodes");
        assert_eq!(encoded.len(), 16 + 2 * 44);
        assert_eq!(PointerBatch::decode(&encoded).expect("batch decodes"), batch);
    }

    #[test]
    fn rejects_wrong_version_and_truncation() {
        let mut encoded = PointerBatch {
            batch_sequence: 1,
            client_send_time_ms: 0.0,
            samples: vec![sample()],
        }
        .encode()
        .expect("batch encodes");
        encoded[0] = 9;
        assert_eq!(PointerBatch::decode(&encoded), Err(ProtocolError::InvalidVersion(9)));
        encoded[0] = PROTOCOL_VERSION;
        encoded.pop();
        assert!(matches!(
            PointerBatch::decode(&encoded),
            Err(ProtocolError::Truncated { .. })
        ));
    }

    #[test]
    fn clamps_untrusted_numeric_fields() {
        let dirty = PointerSample {
            x_norm: -5.0,
            y_norm: 9.0,
            pressure: f32::NAN,
            tilt_x_deg: -1000.0,
            tilt_y_deg: 1000.0,
            twist_deg: -15.0,
            ..sample()
        }
        .sanitized();
        assert_eq!((dirty.x_norm, dirty.y_norm, dirty.pressure), (0.0, 1.0, 0.0));
        assert_eq!(
            (dirty.tilt_x_deg, dirty.tilt_y_deg, dirty.twist_deg),
            (-90.0, 90.0, 345.0)
        );
        assert_eq!(
            PointerSample {
                pressure: 0.5,
                ..sample()
            }
            .pressure_u32(),
            512
        );
    }
}
