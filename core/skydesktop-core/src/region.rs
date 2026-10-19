//! Раздел 4.3 плана — регионное кодирование (Region-Based Encoding).

/// Прямоугольная область кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Тип контента в регионе.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    /// Без изменений — можно не кодировать.
    Static,
    /// Текстовые документы — near-lossless.
    Text,
    /// Видео контент.
    Video,
    /// Игры.
    Game,
    /// Смешанный контент.
    Mixed,
}

/// Приоритет кодирования региона.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

/// Параметры кодирования региона.
#[derive(Debug, Clone)]
pub struct RegionEncodingParams {
    pub qp: u8,
    pub chroma: crate::codec::Chroma,
    pub skip: bool,
}

/// Регион кадра.
#[derive(Debug, Clone)]
pub struct Region {
    pub bounds: Rect,
    pub content_type: ContentType,
    pub priority: Priority,
    pub params: RegionEncodingParams,
}

/// Регионный энкодер: разные параметры для разных областей экрана.
pub struct RegionBasedEncoder {
    regions: Vec<Region>,
}

impl RegionBasedEncoder {
    pub fn new() -> Self {
        Self { regions: Vec::new() }
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    /// Сегментация кадра на регионы (заглушка — в реальном ядре здесь
    /// детекция текста/движения и motion estimation).
    pub fn segment_frame(&mut self, regions: Vec<(Rect, ContentType, Priority)>) {
        self.regions = regions
            .into_iter()
            .map(|(bounds, content_type, priority)| Region {
                bounds,
                content_type,
                priority,
                params: RegionEncodingParams {
                    qp: 20,
                    chroma: crate::codec::Chroma::Yuv420,
                    skip: false,
                },
            })
            .collect();
    }

    /// Применение параметров кодирования по типу контента (план 4.3).
    pub fn apply_content_params(&mut self) {
        for region in &mut self.regions {
            match region.content_type {
                ContentType::Text => {
                    region.params.qp = 10; // near-lossless
                    region.params.chroma = crate::codec::Chroma::Yuv444;
                    region.params.skip = false;
                    region.priority = Priority::Critical;
                }
                ContentType::Video => {
                    region.params.qp = 26;
                    region.params.chroma = crate::codec::Chroma::Yuv420;
                    region.params.skip = false;
                    region.priority = Priority::Normal;
                }
                ContentType::Game => {
                    region.params.qp = 22;
                    region.params.chroma = crate::codec::Chroma::Yuv420;
                    region.params.skip = false;
                    region.priority = Priority::High;
                }
                ContentType::Static => {
                    region.params.skip = true;
                    region.priority = Priority::Low;
                }
                ContentType::Mixed => {
                    region.params.qp = 20;
                    region.params.chroma = crate::codec::Chroma::Yuv420;
                    region.params.skip = false;
                    region.priority = Priority::Normal;
                }
            }
        }
    }
}

impl Default for RegionBasedEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_region_is_near_lossless() {
        let mut enc = RegionBasedEncoder::new();
        enc.segment_frame(vec![(
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 40,
            },
            ContentType::Text,
            Priority::Normal,
        )]);
        enc.apply_content_params();

        let region = &enc.regions()[0];
        assert_eq!(region.params.qp, 10);
        assert_eq!(region.params.chroma, crate::codec::Chroma::Yuv444);
        assert!(!region.params.skip);
    }

    #[test]
    fn static_region_is_skipped() {
        let mut enc = RegionBasedEncoder::new();
        enc.segment_frame(vec![(
            Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            ContentType::Static,
            Priority::Low,
        )]);
        enc.apply_content_params();
        assert!(enc.regions()[0].params.skip);
    }
}
