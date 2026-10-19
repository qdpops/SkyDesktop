//! Обмен сообщениями между окнами приложения.
//!
//! Сеанс и менеджер подключений — отдельные процессы: они сообщают панели
//! список клиентов, метрики и имя удалённого компьютера обычными POST-ами.
//! Панель раскладывает это в шину событий, из которой читает окно.

use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};

use skydesktop_bridge::{CoreEvent, IncomingClientDto, SessionMetricsDto};

use crate::engine::{Clients, EventBus};

#[derive(Clone)]
pub struct BridgeState {
    pub bus: EventBus,
    /// Текущие подключения; заполняются только в режиме менеджера подключений.
    pub clients: Clients,
}

/// Роутер моста поверх живого ядра.
pub fn router(state: BridgeState) -> Router {
    Router::new()
        // Сюда окна сеансов присылают то, что измерило ядро.
        .route("/session-metrics", post(session_metrics))
        // А менеджер подключений — список тех, кто подключён к компьютеру.
        .route("/clients", post(clients))
        // Сеанс сообщает, что удалённая сторона потребовала пароль.
        .route("/password-required", post(password_required))
        // И имя компьютера, к которому подключились.
        .route("/peer-name", post(peer_name))
        // Сеанс не состоялся — панель покажет причину.
        .route("/session-failed", post(session_failed))
        .with_state(state)
}

async fn clients(
    State(state): State<BridgeState>,
    Json(report): Json<ClientsReport>,
) -> impl IntoResponse {
    crate::control::set(report.control);
    let list = report.clients;
    let known: Vec<i32> = state.clients.lock().unwrap().iter().map(|c| c.conn_id).collect();
    for conn_id in known {
        if !list.iter().any(|c| c.conn_id == conn_id) {
            state.bus.publish(CoreEvent::ConnectionClosed { conn_id });
        }
    }
    for client in &list {
        state.bus.publish(CoreEvent::IncomingConnection {
            client: client.clone(),
        });
    }
    *state.clients.lock().unwrap() = list;
    axum::http::StatusCode::NO_CONTENT
}

#[derive(serde::Deserialize)]
struct ClientsReport {
    control: String,
    clients: Vec<IncomingClientDto>,
}

#[derive(serde::Deserialize)]
struct PasswordRequired {
    address: String,
    wrong: bool,
}

async fn password_required(
    State(state): State<BridgeState>,
    Json(request): Json<PasswordRequired>,
) -> impl IntoResponse {
    if request.wrong {
        state.bus.publish(CoreEvent::Notice {
            level: "error".to_string(),
            text: "Пароль не подошёл".to_string(),
        });
    }
    state.bus.publish(CoreEvent::PasswordRequired {
        address: request.address,
    });
    axum::http::StatusCode::NO_CONTENT
}

#[derive(serde::Deserialize)]
struct PeerName {
    address: String,
    name: String,
}

async fn peer_name(
    State(state): State<BridgeState>,
    Json(request): Json<PeerName>,
) -> impl IntoResponse {
    state.bus.publish(CoreEvent::PeerName {
        address: request.address,
        name: request.name,
    });
    axum::http::StatusCode::NO_CONTENT
}

#[derive(serde::Deserialize)]
struct SessionFailed {
    address: String,
    reason: String,
}

async fn session_failed(
    State(state): State<BridgeState>,
    Json(request): Json<SessionFailed>,
) -> impl IntoResponse {
    state.bus.publish(CoreEvent::Notice {
        level: "error".to_string(),
        // Адрес показываем так же, как в окне: по три цифры.
        text: format!("{} — {}", crate::panel::format_id(&request.address), request.reason),
    });
    axum::http::StatusCode::NO_CONTENT
}

async fn session_metrics(
    State(state): State<BridgeState>,
    Json(metrics): Json<SessionMetricsDto>,
) -> impl IntoResponse {
    state.bus.publish(CoreEvent::SessionMetrics { metrics });
    axum::http::StatusCode::NO_CONTENT
}
