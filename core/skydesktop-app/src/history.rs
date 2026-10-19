//! История подключений.
//!
//! Список того, к чему подключались: адрес, имя компьютера (его сообщает
//! сеанс) и когда это было. Хранится рядом с настройками приложения.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Сколько записей помним.
const LIMIT: usize = 50;

/// Одно посещение.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Visit {
    pub address: String,
    /// Имя компьютера; пусто, пока сеанс его не сообщил.
    #[serde(default)]
    pub name: String,
    /// Когда подключались, в виде «28.08.2026, 16:08».
    #[serde(default)]
    pub when: String,
}

fn path() -> PathBuf {
    let mut path = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    path.push("SkyDesktop");
    let _ = std::fs::create_dir_all(&path);
    path.push("history.json");
    path
}

pub fn load() -> Vec<Visit> {
    std::fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(history: &[Visit]) {
    if let Ok(json) = serde_json::to_vec_pretty(history) {
        let _ = std::fs::write(path(), json);
    }
}

/// Поднимает адрес наверх списка и обновляет время.
pub fn remember(history: &mut Vec<Visit>, address: &str) {
    let normalized = normalize(address);
    let name = history
        .iter()
        .find(|v| normalize(&v.address) == normalized)
        .map(|v| v.name.clone())
        .unwrap_or_default();

    history.retain(|v| normalize(&v.address) != normalized);
    history.insert(
        0,
        Visit {
            address: address.trim().to_string(),
            name,
            when: now(),
        },
    );
    history.truncate(LIMIT);
    save(history);
}

/// Запоминает имя компьютера, которое сообщил сеанс.
pub fn set_name(history: &mut [Visit], address: &str, name: &str) {
    let normalized = normalize(address);
    let mut changed = false;
    for visit in history.iter_mut() {
        if normalize(&visit.address) == normalized && visit.name != name {
            visit.name = name.to_string();
            changed = true;
        }
    }
    if changed {
        save(history);
    }
}

/// Адрес устройства пишут с пробелами и без — сравниваем по цифрам и символам.
pub fn normalize(address: &str) -> String {
    address.chars().filter(|c| !c.is_whitespace()).collect()
}

fn now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    // Локальное время без внешних зависимостей: смещение берём у системы.
    let local = now + local_offset_seconds();
    let (year, month, day, hour, minute) = civil_from_unix(local);
    format!("{day:02}.{month:02}.{year}, {hour:02}:{minute:02}")
}

#[cfg(windows)]
fn local_offset_seconds() -> i64 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

    let mut info: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let result = unsafe { GetTimeZoneInformation(&mut info) };
    // 2 — сейчас действует летнее время.
    let bias = info.Bias + if result == 2 { info.DaylightBias } else { info.StandardBias };
    -(bias as i64) * 60
}

#[cfg(not(windows))]
fn local_offset_seconds() -> i64 {
    0
}

/// Разбор секунд эпохи в дату — по алгоритму Хауарда Хиннанта.
fn civil_from_unix(seconds: i64) -> (i64, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };

    (year, month, day, (rem / 3600) as u32, (rem % 3600 / 60) as u32)
}
