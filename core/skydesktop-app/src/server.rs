//! Локальный HTTP-сервер оконного приложения.
//!
//! Через него окна приложения разговаривают друг с другом: сеанс шлёт панели
//! метрики, менеджер подключений — список клиентов. Интерфейс рисуется
//! нативно, поэтому раздавать статику больше не нужно.

use axum::Router;
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// Предпочтительный порт — тот же, что у dev-сервера моста,
/// чтобы адреса в дев- и релиз-режимах совпадали.
pub const PREFERRED_PORT: u16 = 9080;

/// Поднимает сервер и возвращает фактический адрес.
///
/// Если предпочтительный порт занят (уже открыто главное окно или запущен
/// dev-мост), берётся любой свободный — UI узнаёт адрес из `window.location`.
pub async fn bind_preferring(preferred: Option<u16>) -> std::io::Result<(TcpListener, SocketAddr)> {
    let listener = match preferred {
        Some(port) => match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => listener,
            Err(err) => {
                tracing::warn!("порт {port} занят ({err}), берём свободный");
                TcpListener::bind(("127.0.0.1", 0)).await?
            }
        },
        None => TcpListener::bind(("127.0.0.1", 0)).await?,
    };
    let addr = listener.local_addr()?;
    Ok((listener, addr))
}

/// Обслуживает подключения до завершения процесса.
pub async fn serve(listener: TcpListener, bridge: Router) {
    if let Err(err) = axum::serve(listener, bridge).await {
        tracing::error!("сервер приложения остановлен: {err}");
    }
}
