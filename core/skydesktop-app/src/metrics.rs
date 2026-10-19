//! Передача показателей сеанса из окна вьюера в панель управления.
//!
//! Сеанс живёт отдельным процессом, поэтому измеренные ядром скорость,
//! задержка и выбранный кодек отправляются в панель обычным HTTP-запросом
//! на её локальный сервер. Панель их только показывает; если она закрыта,
//! отправка молча пропускается.

use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::time::Duration;

use skydesktop_bridge::{IncomingClientDto, SessionMetricsDto};

/// Куда шлём: панель занимает этот порт первой (см. `server::PREFERRED_PORT`).
const PANEL_BASE: &str = "http://127.0.0.1:9080";

const TIMEOUT: Duration = Duration::from_secs(2);

/// Что отправляем панели.
enum Report {
    /// Показатели идущего сеанса.
    Metrics(SessionMetricsDto),
    /// Список подключённых к этому компьютеру и адрес для команд.
    Clients {
        control: String,
        clients: Vec<IncomingClientDto>,
    },
    /// Удалённая сторона потребовала пароль.
    PasswordRequired { address: String, wrong: bool },
    /// Имя компьютера, к которому подключились.
    PeerName { address: String, name: String },
}

static SENDER: OnceLock<Sender<Report>> = OnceLock::new();

/// Отправляет показатели сеанса в панель.
pub fn report(metrics: SessionMetricsDto) {
    send(Report::Metrics(metrics));
}

/// Отправляет панели текущий список подключений.
///
/// `control` — адрес, по которому панель просит разорвать подключение.
pub fn report_clients(control: String, clients: Vec<IncomingClientDto>) {
    send(Report::Clients { control, clients });
}

/// Сообщает панели, что подключиться не удалось.
///
/// Отправляем сразу, не через общую очередь: окно сеанса закрывается через
/// доли секунды после ошибки, и фоновый поток не успел бы её отправить —
/// подключение так и выглядело бы «мигнуло и ничего».
pub fn report_failed(address: String, reason: String) {
    let worker = std::thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder().timeout(TIMEOUT).build() {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!("причину отказа отправить не удалось: {err}");
                return;
            }
        };
        if let Err(err) = client
            .post(format!("{PANEL_BASE}/session-failed"))
            .json(&serde_json::json!({ "address": address, "reason": reason }))
            .send()
        {
            tracing::warn!("панель не приняла причину отказа: {err}");
        }
    });

    // Ждём отправку: дальше процесс завершится и поток оборвётся.
    let _ = worker.join();
}

/// Сообщает панели имя компьютера, к которому подключились.
pub fn report_peer_name(address: String, name: String) {
    send(Report::PeerName { address, name });
}

/// Сообщает панели, что удалённая сторона просит пароль.
///
/// `wrong` — прошлый пароль не подошёл.
pub fn report_password_required(address: String, wrong: bool) {
    send(Report::PasswordRequired { address, wrong });
}

/// Вызывается из потока ядра, поэтому сама отправка вынесена в отдельный
/// поток: внутри рантайма tokio блокирующий HTTP-клиент работать не будет.
fn send(report: Report) {
    let sender = SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Report>();
        std::thread::Builder::new()
            .name("skydesktop-report".to_string())
            .spawn(move || {
                let client = match reqwest::blocking::Client::builder().timeout(TIMEOUT).build() {
                    Ok(client) => client,
                    Err(err) => {
                        tracing::warn!("панель не будет получать состояние: {err}");
                        return;
                    }
                };
                while let Ok(report) = rx.recv() {
                    let _ = match report {
                        Report::Metrics(metrics) => client
                            .post(format!("{PANEL_BASE}/session-metrics"))
                            .json(&metrics)
                            .send(),
                        Report::Clients { control, clients } => client
                            .post(format!("{PANEL_BASE}/clients"))
                            .json(&serde_json::json!({
                                "control": control,
                                "clients": clients,
                            }))
                            .send(),
                        Report::PasswordRequired { address, wrong } => client
                            .post(format!("{PANEL_BASE}/password-required"))
                            .json(&serde_json::json!({ "address": address, "wrong": wrong }))
                            .send(),
                        Report::PeerName { address, name } => client
                            .post(format!("{PANEL_BASE}/peer-name"))
                            .json(&serde_json::json!({ "address": address, "name": name }))
                            .send(),
                    };
                }
            })
            .expect("не удалось запустить поток отправки состояния");
        tx
    });

    let _ = sender.send(report);
}
