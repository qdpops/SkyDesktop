//! Каталог нод: откуда приложение узнаёт актуальный список серверов-посредников.
//!
//! Нода регистрируется в каталоге сама (см. `server/directory/`), а приложение
//! периодически забирает оттуда список и кладёт его в настройки ядра. Дальше
//! работает штатный механизм: хост регистрируется сразу на всех серверах из
//! списка, а для исходящих подключений выбирается самый быстрый.
//!
//! Запрос делается блокирующим клиентом в отдельном коротком потоке: поток
//! ядра не должен вставать на время сетевого обмена, а асинхронный клиент
//! потребовал бы рантайма там, где рантайма быть не должно.

use std::time::Duration;

use serde::Deserialize;

/// Сколько ждём ответа каталога.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Ответ каталога.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Document {
    /// Серверы-посредники: `host` либо `host:порт`.
    pub rendezvous: Vec<String>,
    /// Реле. Показываются справочно: реле назначает сам посредник.
    pub relay: Vec<String>,
    /// Общий публичный ключ пула; пусто — не менять текущий.
    pub key: String,
}

/// Ответ каталога на запрос адреса.
#[derive(Debug, Deserialize)]
struct Issued {
    id: String,
}

/// Просит у каталога постоянный адрес для этой машины.
///
/// Адрес закреплён за железом: каталог отдаёт один и тот же при каждом
/// запуске. Без этого ядро придумывает адрес само и меняет его при любом
/// расхождении с сервером — записанный у пользователя адрес переставал
/// работать.
pub fn claim_id(url: &str, machine: &str) -> Result<String, String> {
    let base = normalize(url);
    // Каталог отдаёт список нод по адресу вида `.../nodes`; адреса живут
    // рядом, по `/id`.
    let endpoint = match base.rsplit_once('/') {
        Some((head, _)) => format!("{head}/id"),
        None => format!("{base}/id"),
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|err| format!("не удалось создать HTTP-клиент: {err}"))?;

    let response = client
        .post(&endpoint)
        .json(&serde_json::json!({ "machine": machine }))
        .send()
        .map_err(|err| format!("каталог недоступен: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("каталог ответил {status}"));
    }

    let issued: Issued = response
        .json()
        .map_err(|err| format!("каталог вернул неожиданный ответ: {err}"))?;

    if issued.id.is_empty() || !issued.id.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("каталог вернул неподходящий адрес «{}»", issued.id));
    }
    Ok(issued.id)
}

/// Забирает список нод из каталога.
///
/// Вызывать из обычного потока: клиент блокирующий и внутри рантайма tokio
/// работать не будет.
pub fn fetch(url: &str) -> Result<Document, String> {
    let url = normalize(url);
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|err| format!("не удалось создать HTTP-клиент: {err}"))?;

    let response = client
        .get(&url)
        .header("accept", "application/json")
        .send()
        .map_err(|err| format!("каталог недоступен: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("каталог ответил {status}"));
    }

    let document: Document = response
        .json()
        .map_err(|err| format!("каталог вернул неожиданный ответ: {err}"))?;

    if document.rendezvous.is_empty() {
        return Err("каталог не вернул ни одного сервера".to_string());
    }
    Ok(document)
}

/// Пользователь вводит адрес каталога как угодно — приводим к полному URL.
fn normalize(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_host_becomes_https_url() {
        assert_eq!(normalize("nodes.example.com/"), "https://nodes.example.com");
    }

    #[test]
    fn explicit_scheme_is_kept() {
        assert_eq!(
            normalize("http://127.0.0.1:8787/nodes"),
            "http://127.0.0.1:8787/nodes"
        );
    }
}
