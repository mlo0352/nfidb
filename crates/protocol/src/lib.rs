//! Wire types and coordinate transforms shared by the NFiDB host.

mod mapping;
mod packet;

pub use mapping::{FitMode, NormalizedPoint, PixelPoint, TargetGeometry, VideoContentRect, content_rect};
pub use packet::{Action, DeviceType, PROTOCOL_VERSION, PointerBatch, PointerSample, ProtocolError};
