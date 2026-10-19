//! Раздел 4.1 плана — оптимизация кодеков.

/// Кодеки, поддерживаемые ядром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// AV1 — максимальное качество (SVT-AV1 / rav1e).
    AV1,
    /// H.265/HEVC — аппаратное ускорение и баланс.
    H265,
    /// H.264/AVC — совместимость и слабые каналы.
    H264,
}

/// Цветовая субдискретизация.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// 4:4:4 — без потерь цветности, чёткий текст.
    Yuv444,
    /// 4:2:0 — эффективнее по битрейту.
    Yuv420,
}

/// Конфигурация кодирования (аналог `CodecConfig` из плана).
#[derive(Debug, Clone)]
pub struct CodecConfig {
    pub codec: Codec,
    pub chroma: Chroma,
    /// Constant Rate Factor: 14–18 для near-lossless, 22–28 для сжатия.
    pub crf: u8,
    /// Скорость кодирования (0 — медленно/максимальное качество, 10 — быстро).
    pub speed: u8,
    /// Интервал ключевых кадров.
    pub keyframe_interval: u32,
    /// Количество B-кадров.
    pub b_frames: u32,
    /// Количество опорных кадров.
    pub reference_frames: u32,
    /// Битовая глубина (8 или 10).
    pub bit_depth: u8,
    /// Аппаратное ускорение (NVENC/QuickSync/VCE/VideoToolbox выбирается автоматически).
    pub hardware_accel: bool,
}

impl CodecConfig {
    /// Максимальное качество: near-lossless, максимум опорных кадров.
    pub fn max_quality(codec: Codec) -> Self {
        Self {
            codec,
            chroma: Chroma::Yuv444,
            crf: 14,
            speed: 0,
            keyframe_interval: 30,
            b_frames: 3,
            reference_frames: 16,
            bit_depth: 10,
            hardware_accel: true,
        }
    }

    /// Сжатие для слабых каналов: низкий битрейт, минимальная задержка.
    pub fn low_latency() -> Self {
        Self {
            codec: Codec::H264,
            chroma: Chroma::Yuv420,
            crf: 28,
            speed: 8,
            keyframe_interval: 60,
            b_frames: 0,
            reference_frames: 4,
            bit_depth: 8,
            hardware_accel: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_quality_uses_444_and_10bit() {
        let cfg = CodecConfig::max_quality(Codec::AV1);
        assert_eq!(cfg.chroma, Chroma::Yuv444);
        assert_eq!(cfg.bit_depth, 10);
        assert!(cfg.crf <= 18);
    }

    #[test]
    fn low_latency_uses_420_and_h264() {
        let cfg = CodecConfig::low_latency();
        assert_eq!(cfg.codec, Codec::H264);
        assert_eq!(cfg.chroma, Chroma::Yuv420);
        assert_eq!(cfg.b_frames, 0);
    }
}
