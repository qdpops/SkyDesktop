//! Настройки сеанса — мост между UI и ядром.

use crate::codec::{Chroma, Codec};
use crate::quality::{AdaptiveQualityController, ChannelMetrics, QualityDecision};

/// Профили качества, как в интерфейсе SkyDesktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityProfile {
    /// Автоматическая адаптация под качество канала (рекомендуется).
    Auto,
    /// Максимальное качество.
    Max,
    /// Сбалансированный.
    Balanced,
    /// Низкая задержка.
    LowLatency,
}

/// Настройки сеанса, приходящие из UI.
#[derive(Debug, Clone)]
pub struct SessionSettings {
    pub profile: QualityProfile,
    pub codec: Codec,
    pub chroma: Chroma,
    pub bitrate_mbps: u32,
    pub fps: u8,
    pub hdr: bool,
    pub fec: bool,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            profile: QualityProfile::Auto,
            codec: Codec::AV1,
            chroma: Chroma::Yuv444,
            bitrate_mbps: 120,
            fps: 60,
            hdr: true,
            fec: true,
        }
    }
}

impl SessionSettings {
    /// Решение о параметрах кодирования для текущих метрик канала.
    ///
    /// В режиме `Auto` параметры пересчитываются адаптивным контроллером.
    /// В ручных режимах используется фиксированный профиль.
    pub fn decide(&self, metrics: ChannelMetrics) -> QualityDecision {
        if self.profile == QualityProfile::Auto {
            return AdaptiveQualityController::new(metrics).optimize();
        }

        match self.profile {
            QualityProfile::Max => QualityDecision {
                codec: Codec::AV1,
                chroma: Chroma::Yuv444,
                bitrate_mbps: self.bitrate_mbps,
                fps: self.fps,
                hdr: self.hdr,
                fec: self.fec,
            },
            QualityProfile::Balanced => QualityDecision {
                codec: Codec::H265,
                chroma: Chroma::Yuv420,
                bitrate_mbps: self.bitrate_mbps,
                fps: self.fps.min(45),
                hdr: self.hdr,
                fec: self.fec,
            },
            QualityProfile::LowLatency => QualityDecision {
                codec: Codec::H264,
                chroma: Chroma::Yuv420,
                bitrate_mbps: self.bitrate_mbps,
                fps: self.fps.min(30),
                hdr: false,
                fec: true,
            },
            QualityProfile::Auto => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_profile_adapts_to_channel() {
        let settings = SessionSettings::default();
        let excellent = ChannelMetrics::new(480.0, 8.0, 0.1, 2.0);
        let poor = ChannelMetrics::new(4.0, 100.0, 7.0, 30.0);

        let best = settings.decide(excellent);
        let compressed = settings.decide(poor);

        assert_eq!(best.codec, Codec::AV1);
        assert_eq!(best.chroma, Chroma::Yuv444);
        assert_eq!(compressed.codec, Codec::H264);
        assert_eq!(compressed.chroma, Chroma::Yuv420);
        assert!(compressed.bitrate_mbps < best.bitrate_mbps);
    }

    #[test]
    fn manual_profile_is_fixed() {
        let mut settings = SessionSettings::default();
        settings.profile = QualityProfile::Max;
        settings.codec = Codec::AV1;
        let decision = settings.decide(ChannelMetrics::new(1.0, 200.0, 20.0, 50.0));
        assert_eq!(decision.codec, Codec::AV1);
        assert_eq!(decision.fps, 60);
    }
}
