//! SkyDesktop Core — адаптационный слой ядра поверх идей RustDesk.
//!
//! Модули соответствуют разделам плана (План.txt):
//! - `codec`  — конфигурация кодеков (4.1)
//! - `quality`— адаптивный контроллер качества (4.2, 5.1)
//! - `region` — регионное кодирование (4.3)
//! - `network`— сетевые оптимизации, FEC (4.4)
//! - `session`— настройки сеанса, профили качества (мост к UI)

pub mod codec;
pub mod network;
pub mod quality;
pub mod region;
pub mod session;

pub use codec::{Chroma, Codec, CodecConfig};
pub use network::{FecConfig, FecType, NetworkOptimizer};
pub use quality::{AdaptiveQualityController, ChannelLevel, ChannelMetrics, QualityDecision, QualityMetrics};
pub use region::{ContentType, Rect, Region, RegionBasedEncoder};
pub use session::{QualityProfile, SessionSettings};
