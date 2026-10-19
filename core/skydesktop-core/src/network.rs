//! Раздел 4.4 плана — сетевые оптимизации (FEC, приоритизация).

/// Тип Forward Error Correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FecType {
    None,
    Xor,
    ReedSolomon,
}

/// Конфигурация FEC.
#[derive(Debug, Clone, Copy)]
pub struct FecConfig {
    /// Избыточность, %.
    pub redundancy: u8,
    /// Размер блока, пакетов.
    pub block_size: u8,
    pub fec_type: FecType,
}

/// Приоритет отправки пакета.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketPriority {
    Low,
    High,
    Critical,
}

/// Сетевой оптимизатор: адаптивный FEC и приоритизация кадров.
#[derive(Debug, Clone)]
pub struct NetworkOptimizer {
    pub packet_loss_pct: f32,
}

impl NetworkOptimizer {
    pub fn new(packet_loss_pct: f32) -> Self {
        Self { packet_loss_pct }
    }

    /// Адаптивный выбор FEC по потерям пакетов (план 4.4):
    /// > 5% — Reed-Solomon с избыточностью 20%,
    /// > 1% — XOR с избыточностью 10%,
    /// иначе — без FEC.
    pub fn adaptive_fec(&self) -> FecConfig {
        if self.packet_loss_pct > 5.0 {
            FecConfig {
                redundancy: 20,
                block_size: 8,
                fec_type: FecType::ReedSolomon,
            }
        } else if self.packet_loss_pct > 1.0 {
            FecConfig {
                redundancy: 10,
                block_size: 16,
                fec_type: FecType::Xor,
            }
        } else {
            FecConfig {
                redundancy: 0,
                block_size: 32,
                fec_type: FecType::None,
            }
        }
    }

    /// Приоритет кадра: I-кадры критичны, важные P-кадры — высокий приоритет,
    /// остальное — низкий (план 4.4).
    pub fn frame_priority(is_keyframe: bool, importance: f32) -> PacketPriority {
        if is_keyframe {
            PacketPriority::Critical
        } else if importance > 0.7 {
            PacketPriority::High
        } else {
            PacketPriority::Low
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_loss_uses_reed_solomon() {
        let opt = NetworkOptimizer::new(6.0);
        let fec = opt.adaptive_fec();
        assert_eq!(fec.fec_type, FecType::ReedSolomon);
        assert_eq!(fec.redundancy, 20);
    }

    #[test]
    fn low_loss_uses_no_fec() {
        let opt = NetworkOptimizer::new(0.5);
        let fec = opt.adaptive_fec();
        assert_eq!(fec.fec_type, FecType::None);
        assert_eq!(fec.redundancy, 0);
    }

    #[test]
    fn keyframe_is_critical() {
        assert_eq!(NetworkOptimizer::frame_priority(true, 0.0), PacketPriority::Critical);
        assert_eq!(NetworkOptimizer::frame_priority(false, 0.9), PacketPriority::High);
        assert_eq!(NetworkOptimizer::frame_priority(false, 0.3), PacketPriority::Low);
    }
}
