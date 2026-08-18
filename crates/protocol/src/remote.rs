use serde::{Deserialize, Serialize};

use crate::{PROTOCOL_VERSION, PointerBatch, ProtocolError};

pub const MESSAGE_WHEEL: u8 = 2;
pub const MESSAGE_KEYBOARD: u8 = 3;
pub const MESSAGE_TEXT: u8 = 4;
pub const MESSAGE_COMMAND: u8 = 5;
const WHEEL_BYTES: usize = 32;
const KEYBOARD_HEADER_BYTES: usize = 24;
const TEXT_HEADER_BYTES: usize = 20;
const COMMAND_BYTES: usize = 16;
const MAX_KEY_FIELD_BYTES: usize = 64;
const MAX_TEXT_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub enum InputMessage {
    Pointer(PointerBatch),
    Wheel(WheelInput),
    Keyboard(KeyboardInput),
    Text(TextInput),
    Command(CommandInput),
}

impl InputMessage {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < 2 {
            return Err(ProtocolError::Truncated {
                expected: 2,
                actual: bytes.len(),
            });
        }
        if bytes[0] != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidVersion(bytes[0]));
        }
        match bytes[1] {
            1 => PointerBatch::decode(bytes).map(Self::Pointer),
            MESSAGE_WHEEL => WheelInput::decode(bytes).map(Self::Wheel),
            MESSAGE_KEYBOARD => KeyboardInput::decode(bytes).map(Self::Keyboard),
            MESSAGE_TEXT => TextInput::decode(bytes).map(Self::Text),
            MESSAGE_COMMAND => CommandInput::decode(bytes).map(Self::Command),
            other => Err(ProtocolError::InvalidMessageType(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WheelInput {
    pub modifiers: u16,
    pub sequence: u32,
    pub x_norm: f32,
    pub y_norm: f32,
    pub delta_x: f32,
    pub delta_y: f32,
    pub client_time_ms: f64,
}

impl WheelInput {
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.x_norm = finite_f32(self.x_norm).clamp(0.0, 1.0);
        self.y_norm = finite_f32(self.y_norm).clamp(0.0, 1.0);
        self.delta_x = finite_f32(self.delta_x).clamp(-10_000.0, 10_000.0);
        self.delta_y = finite_f32(self.delta_y).clamp(-10_000.0, 10_000.0);
        self.client_time_ms = finite_f64(self.client_time_ms);
        self
    }

    pub fn encode(self) -> Vec<u8> {
        let value = self.sanitized();
        let mut bytes = Vec::with_capacity(WHEEL_BYTES);
        bytes.extend_from_slice(&[PROTOCOL_VERSION, MESSAGE_WHEEL]);
        put_u16(&mut bytes, value.modifiers);
        put_u32(&mut bytes, value.sequence);
        put_f32(&mut bytes, value.x_norm);
        put_f32(&mut bytes, value.y_norm);
        put_f32(&mut bytes, value.delta_x);
        put_f32(&mut bytes, value.delta_y);
        put_f64(&mut bytes, value.client_time_ms);
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        exact_len(bytes, WHEEL_BYTES)?;
        let mut cursor = Cursor::after_header(bytes);
        Ok(Self {
            modifiers: cursor.u16()?,
            sequence: cursor.u32()?,
            x_norm: cursor.f32()?,
            y_norm: cursor.f32()?,
            delta_x: cursor.f32()?,
            delta_y: cursor.f32()?,
            client_time_ms: cursor.f64()?,
        }
        .sanitized())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum KeyAction {
    Down = 1,
    Up = 2,
}

impl TryFrom<u8> for KeyAction {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Down),
            2 => Ok(Self::Up),
            other => Err(ProtocolError::InvalidKeyboardAction(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardInput {
    pub action: KeyAction,
    pub location: u8,
    pub repeat: bool,
    pub modifiers: u16,
    pub sequence: u32,
    pub client_time_ms: u64,
    pub code: String,
    pub key: String,
}

impl KeyboardInput {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let code = self.code.as_bytes();
        let key = self.key.as_bytes();
        validate_key_fields(code, key)?;
        let mut bytes = Vec::with_capacity(KEYBOARD_HEADER_BYTES + code.len() + key.len());
        bytes.extend_from_slice(&[PROTOCOL_VERSION, MESSAGE_KEYBOARD, self.action as u8, self.location]);
        put_u16(&mut bytes, self.modifiers);
        bytes.push(u8::from(self.repeat));
        bytes.push(0);
        put_u32(&mut bytes, self.sequence);
        put_u64(&mut bytes, self.client_time_ms);
        put_u16(&mut bytes, code.len() as u16);
        put_u16(&mut bytes, key.len() as u16);
        bytes.extend_from_slice(code);
        bytes.extend_from_slice(key);
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        minimum_len(bytes, KEYBOARD_HEADER_BYTES)?;
        let mut cursor = Cursor::after_header(bytes);
        let action = KeyAction::try_from(cursor.u8()?)?;
        let location = cursor.u8()?;
        let modifiers = cursor.u16()?;
        let repeat = cursor.u8()? != 0;
        let _reserved = cursor.u8()?;
        let sequence = cursor.u32()?;
        let client_time_ms = cursor.u64()?;
        let code_len = usize::from(cursor.u16()?);
        let key_len = usize::from(cursor.u16()?);
        validate_key_lengths(code_len, key_len)?;
        exact_len(bytes, KEYBOARD_HEADER_BYTES + code_len + key_len)?;
        let code = utf8(cursor.take_slice(code_len)?)?;
        let key = utf8(cursor.take_slice(key_len)?)?;
        Ok(Self {
            action,
            location,
            repeat,
            modifiers,
            sequence,
            client_time_ms,
            code,
            key,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextInput {
    pub sequence: u32,
    pub client_time_ms: u64,
    pub text: String,
}

impl TextInput {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let text = self.text.as_bytes();
        if text.is_empty() || text.len() > MAX_TEXT_BYTES {
            return Err(ProtocolError::InvalidTextLength(text.len()));
        }
        let mut bytes = Vec::with_capacity(TEXT_HEADER_BYTES + text.len());
        bytes.extend_from_slice(&[PROTOCOL_VERSION, MESSAGE_TEXT, 0, 0]);
        put_u32(&mut bytes, self.sequence);
        put_u64(&mut bytes, self.client_time_ms);
        put_u32(&mut bytes, text.len() as u32);
        bytes.extend_from_slice(text);
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        minimum_len(bytes, TEXT_HEADER_BYTES)?;
        let mut cursor = Cursor::new(bytes, 4);
        let sequence = cursor.u32()?;
        let client_time_ms = cursor.u64()?;
        let text_len = cursor.u32()? as usize;
        if text_len == 0 || text_len > MAX_TEXT_BYTES {
            return Err(ProtocolError::InvalidTextLength(text_len));
        }
        exact_len(bytes, TEXT_HEADER_BYTES + text_len)?;
        Ok(Self {
            sequence,
            client_time_ms,
            text: utf8(cursor.take_slice(text_len)?)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum RemoteCommand {
    AppNext = 1,
    AppPrevious = 2,
    MinimizeForeground = 3,
    TaskView = 4,
    ResetInput = 5,
}

impl TryFrom<u8> for RemoteCommand {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::AppNext),
            2 => Ok(Self::AppPrevious),
            3 => Ok(Self::MinimizeForeground),
            4 => Ok(Self::TaskView),
            5 => Ok(Self::ResetInput),
            other => Err(ProtocolError::InvalidCommand(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInput {
    pub command: RemoteCommand,
    pub sequence: u32,
    pub client_time_ms: u64,
}

impl CommandInput {
    pub fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(COMMAND_BYTES);
        bytes.extend_from_slice(&[PROTOCOL_VERSION, MESSAGE_COMMAND, self.command as u8, 0]);
        put_u32(&mut bytes, self.sequence);
        put_u64(&mut bytes, self.client_time_ms);
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        exact_len(bytes, COMMAND_BYTES)?;
        let mut cursor = Cursor::new(bytes, 2);
        let command = RemoteCommand::try_from(cursor.u8()?)?;
        let _reserved = cursor.u8()?;
        Ok(Self {
            command,
            sequence: cursor.u32()?,
            client_time_ms: cursor.u64()?,
        })
    }
}

fn finite_f32(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

fn finite_f64(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn validate_key_fields(code: &[u8], key: &[u8]) -> Result<(), ProtocolError> {
    validate_key_lengths(code.len(), key.len())?;
    if code.is_empty() || !code.is_ascii() {
        return Err(ProtocolError::InvalidKeyField);
    }
    Ok(())
}

fn validate_key_lengths(code_len: usize, key_len: usize) -> Result<(), ProtocolError> {
    if code_len == 0 || code_len > MAX_KEY_FIELD_BYTES || key_len > MAX_KEY_FIELD_BYTES {
        return Err(ProtocolError::InvalidKeyField);
    }
    Ok(())
}

fn utf8(bytes: &[u8]) -> Result<String, ProtocolError> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| ProtocolError::InvalidUtf8)
}

fn minimum_len(bytes: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if bytes.len() < expected {
        Err(ProtocolError::Truncated {
            expected,
            actual: bytes.len(),
        })
    } else {
        Ok(())
    }
}

fn exact_len(bytes: &[u8], expected: usize) -> Result<(), ProtocolError> {
    minimum_len(bytes, expected)?;
    if bytes.len() > expected {
        Err(ProtocolError::TrailingBytes(bytes.len() - expected))
    } else {
        Ok(())
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8], position: usize) -> Self {
        Self { bytes, position }
    }

    const fn after_header(bytes: &'a [u8]) -> Self {
        Self::new(bytes, 2)
    }

    fn take_slice(&mut self, count: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self.position.saturating_add(count);
        let value = self.bytes.get(self.position..end).ok_or(ProtocolError::Truncated {
            expected: end,
            actual: self.bytes.len(),
        })?;
        self.position = end;
        Ok(value)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let mut value = [0_u8; N];
        value.copy_from_slice(self.take_slice(N)?);
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

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_le_bytes(self.take()?))
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

fn put_u64(bytes: &mut Vec<u8>, value: u64) {
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

    #[test]
    fn every_remote_message_round_trips() {
        let wheel = WheelInput {
            modifiers: 3,
            sequence: 7,
            x_norm: 0.25,
            y_norm: 0.75,
            delta_x: -1.5,
            delta_y: 24.0,
            client_time_ms: 42.5,
        };
        assert_eq!(InputMessage::decode(&wheel.encode()), Ok(InputMessage::Wheel(wheel)));

        let keyboard = KeyboardInput {
            action: KeyAction::Down,
            location: 1,
            repeat: true,
            modifiers: 6,
            sequence: 8,
            client_time_ms: 43,
            code: "AltLeft".to_owned(),
            key: "Alt".to_owned(),
        };
        assert_eq!(
            InputMessage::decode(&keyboard.encode().expect("keyboard encodes")),
            Ok(InputMessage::Keyboard(keyboard))
        );

        let text = TextInput {
            sequence: 9,
            client_time_ms: 44,
            text: "Hello, 世界".to_owned(),
        };
        assert_eq!(
            InputMessage::decode(&text.encode().expect("text encodes")),
            Ok(InputMessage::Text(text))
        );

        let command = CommandInput {
            command: RemoteCommand::AppNext,
            sequence: 10,
            client_time_ms: 45,
        };
        assert_eq!(
            InputMessage::decode(&command.encode()),
            Ok(InputMessage::Command(command))
        );
    }

    #[test]
    fn rejects_oversized_and_invalid_remote_packets() {
        let text = TextInput {
            sequence: 1,
            client_time_ms: 0,
            text: "x".repeat(MAX_TEXT_BYTES + 1),
        };
        assert_eq!(text.encode(), Err(ProtocolError::InvalidTextLength(MAX_TEXT_BYTES + 1)));

        let mut command = CommandInput {
            command: RemoteCommand::TaskView,
            sequence: 1,
            client_time_ms: 0,
        }
        .encode();
        command[2] = 99;
        assert_eq!(InputMessage::decode(&command), Err(ProtocolError::InvalidCommand(99)));
    }
}
