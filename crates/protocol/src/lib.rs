//! Wire types and coordinate transforms shared by the NFiDB host.

mod mapping;
mod packet;
mod remote;

pub use mapping::{FitMode, NormalizedPoint, PixelPoint, TargetGeometry, VideoContentRect, content_rect};
pub use packet::{Action, DeviceType, PROTOCOL_VERSION, PointerBatch, PointerSample, ProtocolError};
pub use remote::{CommandInput, InputMessage, KeyAction, KeyboardInput, RemoteCommand, TextInput, WheelInput};
