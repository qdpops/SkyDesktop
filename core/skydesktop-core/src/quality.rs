//! Раздел 4.2 и 5.1 плана — адаптивный контроллер качества и метрики.

use crate::codec::{Chroma, Codec};

/// Метрики канала в реальном времени.
#[derive(Debug, Clone, Copy)]
pub struct ChannelMetrics {
    /// Пропускная способность, Мбит/с.
    pub bandwidth_mbps: f32,
    /// Задержка, мс.
    pub latency_ms: f32,
    /// Потери пакетов, %.
    pub packet_loss_pct: f32,
    /// Джиттер, мс.
    pub jitter_ms: f32,
}

/// Уровень качества канала.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLevel {
    Excellent,
    Good,
    Medium,
    Poor,
}

impl ChannelMetrics {
    pub fn new(bandwidth_mbps: f32, latency_ms: f32, packet_loss_pct: f32, jitter_ms: f32) -> Self {
        Self {
            bandwidth_mbps,
            latency_ms,
            packet_loss_pct,
            jitter_ms,
        }
    }

    /// Классификация канала: отличный → максимум качества, слабый → сжатие.
    pub fn level(&self) -> ChannelLevel {
        if self.latency_ms < 20.0 && self.packet_loss_pct < 0.5 && self.bandwidth_mbps > 50.0 {
            ChannelLevel::Excellent
        } else if self.latency_ms < 40.0 && self.packet_loss_pct < 2.0 && self.bandwidth_mbps > 15.0 {
            ChannelLevel::Good
        } else if self.latency_ms < 80.0 && self.packet_loss_pct < 5.0 && self.bandwidth_mbps > 5.0 {
            ChannelLevel::Medium
        } else {
            ChannelLevel::Poor
        }
    }
}

/// Решение контроллера качества.
#[derive(Debug, Clone)]
pub struct QualityDecision {
    pub codec: Codec,
    pub chroma: Chroma,
    /// Целевой битрейт, Мбит/с.
    pub bitrate_mbps: u32,
    pub fps: u8,
    pub hdr: bool,
    pub fec: bool,
}

/// Адаптивный контроллер качества (ML/эвристика, план 4.2).
#[derive(Debug, Clone)]
pub struct AdaptiveQualityController {
    metrics: ChannelMetrics,
}

impl AdaptiveQualityController {
    pub fn new(metrics: ChannelMetrics) -> Self {
        Self { metrics }
    }

    pub fn update_metrics(&mut self, metrics: ChannelMetrics) {
        self.metrics = metrics;
    }

    pub fn metrics(&self) -> ChannelMetrics {
        self.metrics
    }

    /// Выбирает оптимальные параметры под текущее качество канала.
    pub fn optimize(&self) -> QualityDecision {
        let m = &self.metrics;
        match self.metrics.level() {
            // Максимальное качество: AV1, 4:4:4, HDR.
            ChannelLevel::Excellent => QualityDecision {
                codec: Codec::AV1,
                chroma: Chroma::Yuv444,
                bitrate_mbps: (m.bandwidth_mbps * 0.9).clamp(60.0, 200.0) as u32,
                fps: 60,
                hdr: true,
                fec: false,
            },
            // Сбалансированный режим: H.265.
            ChannelLevel::Good => QualityDecision {
                codec: Codec::H265,
                chroma: Chroma::Yuv420,
                bitrate_mbps: (m.bandwidth_mbps * 0.7).clamp(15.0, 80.0) as u32,
                fps: 45,
                hdr: true,
                fec: true,
            },
            ChannelLevel::Medium => QualityDecision {
                codec: Codec::H265,
                chroma: Chroma::Yuv420,
                bitrate_mbps: (m.bandwidth_mbps * 0.6).clamp(5.0, 30.0) as u32,
                fps: 30,
                hdr: false,
                fec: true,
            },
            // Режим низкой задержки: H.264, сжатие, FEC.
            ChannelLevel::Poor => QualityDecision {
                codec: Codec::H264,
                chroma: Chroma::Yuv420,
                bitrate_mbps: (m.bandwidth_mbps * 0.5).clamp(2.0, 10.0) as u32,
                fps: 24,
                hdr: false,
                fec: true,
            },
        }
    }

    /// Прокси VMAF-оценки (0–100) для решения; используется для сравнения режимов.
    pub fn quality_score(&self, decision: &QualityDecision) -> f32 {
        let codec_bonus = match decision.codec {
            Codec::AV1 => 6.0,
            Codec::H265 => 3.0,
            Codec::H264 => 0.0,
        };
        let chroma_bonus = match decision.chroma {
            Chroma::Yuv444 => 4.0,
            Chroma::Yuv420 => 0.0,
        };
        let fps_bonus = (decision.fps as f32 / 60.0) * 3.0;
        let hdr_bonus = if decision.hdr { 1.5 } else { 0.0 };
        let loss_penalty = self.metrics.packet_loss_pct * 1.2;
        (88.0 + codec_bonus + chroma_bonus + fps_bonus + hdr_bonus - loss_penalty).clamp(0.0, 100.0)
    }
}

/// Объективные и субъективные метрики качества (план 5.1).
#[derive(Debug, Clone, Default)]
pub struct QualityMetrics {
    pub vmaf_score: f32,
    pub psnr: f32,
    pub ssim: f32,
    pub text_readability: f32,
    pub color_accuracy: f32,
    pub motion_smoothness: f32,
}

impl QualityMetrics {
    /// Целевые показатели из плана: VMAF > 95, читаемость текста > 0.98, плавность > 0.95.
    pub fn is_target_achieved(&self) -> bool {
        self.vmaf_score > 95.0 && self.text_readability > 0.98 && self.motion_smoothness > 0.95
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics_for(level: ChannelLevel) -> ChannelMetrics {
        match level {
            ChannelLevel::Excellent => ChannelMetrics::new(480.0, 8.0, 0.1, 2.0),
            ChannelLevel::Good => ChannelMetrics::new(60.0, 22.0, 0.7, 5.0),
            ChannelLevel::Medium => ChannelMetrics::new(14.0, 48.0, 2.4, 13.0),
            ChannelLevel::Poor => ChannelMetrics::new(4.5, 96.0, 6.8, 29.0),
        }
    }

    #[test]
    fn excellent_channel_uses_best_quality() {
        let ctl = AdaptiveQualityController::new(metrics_for(ChannelLevel::Excellent));
        let d = ctl.optimize();
        assert_eq!(d.codec, Codec::AV1);
        assert_eq!(d.chroma, Chroma::Yuv444);
        assert_eq!(d.fps, 60);
        assert!(d.hdr);
        assert!(!d.fec);
        assert!(d.bitrate_mbps >= 60);
    }

    #[test]
    fn poor_channel_compresses() {
        let ctl = AdaptiveQualityController::new(metrics_for(ChannelLevel::Poor));
        let d = ctl.optimize();
        assert_eq!(d.codec, Codec::H264);
        assert_eq!(d.chroma, Chroma::Yuv420);
        assert!(d.fps <= 24);
        assert!(d.bitrate_mbps <= 10);
        assert!(d.fec);
    }

    #[test]
    fn channel_levels_classified_correctly() {
        assert_eq!(metrics_for(ChannelLevel::Excellent).level(), ChannelLevel::Excellent);
        assert_eq!(metrics_for(ChannelLevel::Good).level(), ChannelLevel::Good);
        assert_eq!(metrics_for(ChannelLevel::Medium).level(), ChannelLevel::Medium);
        assert_eq!(metrics_for(ChannelLevel::Poor).level(), ChannelLevel::Poor);
    }

    #[test]
    fn quality_targets_from_plan() {
        let m = QualityMetrics {
            vmaf_score: 97.5,
            text_readability: 0.99,
            motion_smoothness: 0.97,
            ..Default::default()
        };
        assert!(m.is_target_achieved());
    }
}
