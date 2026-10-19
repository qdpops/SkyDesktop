//! Мост UI ↔ ядро SkyDesktop.
//!
//! Определяет JSON-протокол, который React-интерфейс использует для общения
//! с Rust-ядром по WebSocket/HTTP:
//!
//! UI → ядро: [`CoreCommand`]
//! Ядро → UI: [`CoreEvent`]
//!
//! Имена полей сериализуются в camelCase — так же, как они объявлены
//! в TypeScript-интерфейсе (`ui/src/types.ts`).

use serde::{Deserialize, Serialize};
use skydesktop_core::quality::{ChannelMetrics, QualityDecision};

/// Профиль качества (совпадает с `QualityProfileId` в UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityProfile {
    Auto,
    Max,
    Balanced,
    LowLatency,
}

impl From<QualityProfile> for skydesktop_core::session::QualityProfile {
    fn from(p: QualityProfile) -> Self {
        match p {
            QualityProfile::Auto => Self::Auto,
            QualityProfile::Max => Self::Max,
            QualityProfile::Balanced => Self::Balanced,
            QualityProfile::LowLatency => Self::LowLatency,
        }
    }
}

/// Кодек.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    /// Кодек выбирает ядро при согласовании с удалённой стороной.
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "AV1")]
    AV1,
    #[serde(rename = "H.265")]
    H265,
    #[serde(rename = "H.264")]
    H264,
}

/// Цветовая субдискретизация.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Chroma {
    #[serde(rename = "4:4:4")]
    I444,
    #[serde(rename = "4:2:0")]
    I420,
}

/// Настройки сеанса, приходящие из UI (`Settings` в `ui/src/types.ts`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettings {
    pub profile: QualityProfile,
    pub codec: Codec,
    pub chroma: Chroma,
    /// Качество изображения в шкале ядра: 50 — обычное, больше — чётче.
    pub quality: u32,
    pub fps: u8,
    pub hdr: bool,
    pub fec: bool,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            profile: QualityProfile::Auto,
            codec: Codec::Auto,
            chroma: Chroma::I444,
            quality: 50,
            fps: 60,
            hdr: true,
            fec: true,
        }
    }
}

impl From<SessionSettings> for skydesktop_core::session::SessionSettings {
    fn from(s: SessionSettings) -> Self {
        Self {
            profile: s.profile.into(),
            codec: match s.codec {
                Codec::Auto | Codec::AV1 => skydesktop_core::codec::Codec::AV1,
                Codec::H265 => skydesktop_core::codec::Codec::H265,
                Codec::H264 => skydesktop_core::codec::Codec::H264,
            },
            chroma: match s.chroma {
                Chroma::I444 => skydesktop_core::codec::Chroma::Yuv444,
                Chroma::I420 => skydesktop_core::codec::Chroma::Yuv420,
            },
            // Модель адаптации оперирует битрейтом; шкала качества ядра
            // примерно вдвое мельче, этого хватает для демо-режима.
            bitrate_mbps: s.quality * 2,
            fps: s.fps,
            hdr: s.hdr,
            fec: s.fec,
        }
    }
}

/// Метрики канала (совпадает с `ChannelMetrics` в UI).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMetricsDto {
    pub bandwidth_mbps: f32,
    pub latency_ms: f32,
    pub packet_loss_pct: f32,
    pub jitter_ms: f32,
}

impl From<ChannelMetricsDto> for ChannelMetrics {
    fn from(m: ChannelMetricsDto) -> Self {
        Self::new(m.bandwidth_mbps, m.latency_ms, m.packet_loss_pct, m.jitter_ms)
    }
}

/// Решение контроллера качества (для отображения в UI).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityDecisionDto {
    pub codec: String,
    pub chroma: String,
    pub bitrate_mbps: u32,
    pub fps: u8,
    pub hdr: bool,
    pub fec: bool,
}

impl From<QualityDecision> for QualityDecisionDto {
    fn from(d: QualityDecision) -> Self {
        Self {
            codec: format!("{:?}", d.codec),
            chroma: match d.chroma {
                skydesktop_core::codec::Chroma::Yuv444 => "4:4:4".to_string(),
                skydesktop_core::codec::Chroma::Yuv420 => "4:2:0".to_string(),
            },
            bitrate_mbps: d.bitrate_mbps,
            fps: d.fps,
            hdr: d.hdr,
            fec: d.fec,
        }
    }
}

/// Команда от UI к ядру.
///
/// `rename_all` переименовывает только имена вариантов, поэтому поля внутри
/// них переименовываются отдельно — интерфейс шлёт их в camelCase.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CoreCommand {
    /// Подключиться к удалённому устройству (адрес из 9 цифр).
    Connect {
        address: String,
        settings: SessionSettings,
        /// Пароль удалённого устройства; пустой — спросит удалённая сторона.
        #[serde(default)]
        password: String,
    },
    /// Принять входящее подключение.
    ///
    /// `conn_id` приходит в [`CoreEvent::IncomingConnection`]; `0` означает
    /// «единственное ожидающее подключение» (режим симуляции).
    AcceptIncoming {
        #[serde(default)]
        conn_id: i32,
    },
    /// Отклонить входящее подключение.
    DeclineIncoming {
        #[serde(default)]
        conn_id: i32,
    },
    /// Разорвать уже установленное входящее подключение.
    CloseConnection { conn_id: i32 },
    /// Завершить активный исходящий сеанс.
    Disconnect,
    /// Обновить настройки.
    UpdateSettings { settings: SessionSettings },
    /// Задать постоянный пароль доступа к этому устройству.
    SetPassword { password: String },
    /// Сгенерировать новый одноразовый пароль.
    RotatePassword,
    /// Выбрать, как разрешается доступ к этому компьютеру.
    SetApproveMode { mode: ApproveMode },
    /// Запросить полное состояние устройства (после подключения UI).
    RefreshState,
}

/// Состояние регистрации устройства на сервере-посреднике.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceStatus {
    /// Соединение с сервером-посредником устанавливается.
    Connecting,
    /// Устройство зарегистрировано и готово принимать подключения.
    Ready,
    /// Сервер отвечает, но адрес за этим компьютером не закреплён:
    /// подключиться к нему по адресу нельзя.
    Unconfirmed,
    /// Нет связи с сервером-посредником.
    Offline,
}

/// Что настроено для доступа к этому устройству.
///
/// В обоих случаях в [`DeviceStateDto::password`] лежит одноразовый пароль:
/// постоянный ядро хранит хешем и наружу не отдаёт.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PasswordKind {
    /// Только одноразовый пароль, меняется при перезапуске.
    Temporary,
    /// Дополнительно задан постоянный пароль.
    Permanent,
}

/// Каталог нод: откуда приложение узнаёт список серверов-посредников.
///
/// Ноды регистрируются в каталоге сами, приложение периодически забирает
/// оттуда список. Кастомный сервер при этом остаётся затравкой пула.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryStateDto {
    /// Адрес каталога (HTTPS), вшитый в сборку. Пусто — каталог не используется.
    pub url: String,
    /// Серверы-посредники, полученные из каталога.
    pub nodes: Vec<String>,
    /// Реле из каталога. Показываются справочно: реле назначает сам посредник.
    pub relays: Vec<String>,
    /// Ошибка последнего опроса; пусто — всё в порядке.
    pub error: String,
}

/// Как этот компьютер разрешает доступ к себе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApproveMode {
    /// Только пароль, подтверждать вручную не нужно.
    Password,
    /// Только подтверждение на этом компьютере, пароль не спрашивается.
    Click,
    /// Пароль, а если он не задан — подтверждение вручную.
    Both,
}

/// Состояние этого устройства: то, что видно в блоке «Это устройство».
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStateDto {
    /// Настоящий адрес устройства (9 цифр).
    pub id: String,
    pub password: String,
    pub password_kind: PasswordKind,
    pub status: DeviceStatus,
    /// Состояние каталога нод.
    pub directory: DirectoryStateDto,
    /// Как разрешается доступ к этому компьютеру.
    pub approve_mode: ApproveMode,
    /// Задержка до сервера связи, мс; `None` — ещё не измерена.
    pub latency_ms: Option<i32>,
    /// Симуляция (демо-мост) или настоящее ядро.
    pub simulated: bool,
    /// Запрашивать права администратора при запуске.
    pub elevate_on_start: bool,
    /// Приложение уже работает с правами администратора.
    pub elevated: bool,
    /// Приложение установлено: подключения принимает служба с полными правами.
    #[serde(default)]
    pub installed: bool,
    /// Идёт установка — ждём ответа в окне UAC.
    #[serde(default)]
    pub installing: bool,
    /// Версия на GitHub, если она новее этой сборки.
    #[serde(default)]
    pub update_version: Option<String>,
    /// Обновление скачивается или ставится.
    #[serde(default)]
    pub updating: bool,
}

/// Входящее подключение к этому устройству.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingClientDto {
    /// Идентификатор подключения в ядре.
    pub conn_id: i32,
    /// Адрес подключающегося устройства.
    pub peer_id: String,
    /// Имя пользователя/устройства, как его сообщила удалённая сторона.
    pub name: String,
    /// Подключение уже разрешено (иначе — ждёт решения).
    pub authorized: bool,
    pub keyboard: bool,
    pub clipboard: bool,
    pub file: bool,
}

/// Показатели идущего сеанса — то, что реально измерило ядро.
///
/// Пустые поля означают «ядро ещё не сообщило», а не ноль.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetricsDto {
    /// Адрес удалённой стороны.
    pub address: String,
    /// Скорость приёма в том виде, в каком её считает ядро («1.2MB/s»).
    pub speed: String,
    /// Задержка, мс.
    pub delay_ms: Option<i32>,
    /// Частота кадров.
    pub fps: Option<i32>,
    /// Целевой битрейт, кбит/с.
    pub target_bitrate_kbps: Option<i32>,
    /// Кодек, выбранный ядром.
    pub codec: String,
    /// Цветовая субдискретизация.
    pub chroma: String,
}

/// Событие от ядра к UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CoreEvent {
    /// Полное состояние этого устройства: адрес, пароль, готовность.
    DeviceState { state: DeviceStateDto },
    /// Кто-то хочет подключиться к нашему устройству — показать окно принятия поверх всех окон.
    IncomingConnection { client: IncomingClientDto },
    /// Входящее подключение завершено или отклонено.
    ConnectionClosed { conn_id: i32 },
    /// Сеанс установлен.
    SessionStarted { name: String, address: String },
    /// Для этого устройства нужен пароль: сохранённого нет.
    PasswordRequired { address: String },
    /// Имя компьютера, к которому подключились: его сообщает сам сеанс.
    PeerName { address: String, name: String },
    /// Кто из знакомых устройств сейчас в сети.
    PeerStates {
        online: Vec<String>,
        offline: Vec<String>,
    },
    /// Сеанс завершён.
    SessionClosed,
    /// Показатели идущего сеанса: скорость, задержка, кодек.
    SessionMetrics { metrics: SessionMetricsDto },
    /// Сообщение пользователю (ошибка подключения, отказ в доступе и т. п.).
    Notice { level: String, text: String },
}

/// Сеанс на стороне ядра: принимает настройки UI и метрики канала,
/// возвращает решение адаптивного контроллера.
pub struct BridgeSession {
    settings: skydesktop_core::session::SessionSettings,
}

impl BridgeSession {
    pub fn new(settings: SessionSettings) -> Self {
        Self {
            settings: settings.into(),
        }
    }

    pub fn update_settings(&mut self, settings: SessionSettings) {
        self.settings = settings.into();
    }

    pub fn decide(&self, metrics: ChannelMetricsDto) -> QualityDecisionDto {
        self.settings.decide(metrics.into()).into()
    }
}

/// Удобная функция: команда UI → JSON-строка (для отладки/тестов).
pub fn command_to_json(cmd: &CoreCommand) -> serde_json::Result<String> {
    serde_json::to_string(cmd)
}

/// Удобная функция: JSON-строка → событие ядра.
pub fn event_from_json(json: &str) -> serde_json::Result<CoreEvent> {
    serde_json::from_str(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_command_serializes_to_camel_case() {
        let cmd = CoreCommand::Connect {
            address: "193 445 812".to_string(),
            settings: SessionSettings::default(),
            password: String::new(),
        };
        let json = command_to_json(&cmd).unwrap();
        assert!(json.contains("\"type\":\"connect\""));
        assert!(json.contains("\"quality\":50"));
        assert!(json.contains("\"address\":\"193 445 812\""));
    }

    #[test]
    fn command_fields_use_camel_case() {
        // Ошибка здесь означает, что интерфейс не сможет разорвать подключение:
        // он шлёт `connId`, а не `conn_id`.
        let json = command_to_json(&CoreCommand::CloseConnection { conn_id: 7 }).unwrap();
        assert!(json.contains("\"connId\":7"), "{json}");

        let event = serde_json::to_string(&CoreEvent::ConnectionClosed { conn_id: 7 }).unwrap();
        assert!(event.contains("\"connId\":7"), "{event}");
    }

    #[test]
    fn incoming_event_parses_from_json() {
        let json = r#"{"type":"incomingConnection","client":{"connId":7,"peerId":"193 445 812","name":"Сервер-1","authorized":false,"keyboard":true,"clipboard":true,"file":true}}"#;
        match event_from_json(json).unwrap() {
            CoreEvent::IncomingConnection { client } => {
                assert_eq!(client.conn_id, 7);
                assert_eq!(client.name, "Сервер-1");
                assert_eq!(client.peer_id, "193 445 812");
                assert!(!client.authorized);
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn auto_session_adapts_to_poor_channel() {
        let session = BridgeSession::new(SessionSettings::default());
        let decision = session.decide(ChannelMetricsDto {
            bandwidth_mbps: 4.0,
            latency_ms: 100.0,
            packet_loss_pct: 7.0,
            jitter_ms: 30.0,
        });
        assert_eq!(decision.codec, "H264");
        assert_eq!(decision.chroma, "4:2:0");
        assert!(decision.fec);
        assert!(decision.bitrate_mbps <= 10);
    }

    #[test]
    fn auto_session_uses_best_on_excellent_channel() {
        let session = BridgeSession::new(SessionSettings::default());
        let decision = session.decide(ChannelMetricsDto {
            bandwidth_mbps: 480.0,
            latency_ms: 8.0,
            packet_loss_pct: 0.1,
            jitter_ms: 2.0,
        });
        assert_eq!(decision.codec, "AV1");
        assert_eq!(decision.chroma, "4:4:4");
        assert!(decision.hdr);
    }
}
