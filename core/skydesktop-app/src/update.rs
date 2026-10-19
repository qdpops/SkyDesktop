//! Обновление с GitHub Releases.
//!
//! Панель время от времени спрашивает GitHub о последнем релизе. Если он
//! новее этой сборки, в боковой панели появляется кнопка «Обновить». Скачанный
//! exe сверяется с контрольной суммой из того же релиза и ставится штатным
//! путём ядра (`update_to`): новая копия получает права администратора,
//! останавливает службу, заменяет файлы в Program Files и запускает всё заново.
//!
//! Релиз обязан нести два файла: [`ASSET`] и [`CHECKSUM`] — их выкладывает
//! `scripts/release.sh`.

use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Репозиторий, откуда берутся релизы.
const REPO: &str = "qdpops/SkyDesktop";
/// Имя exe в релизе. Совпадает с именем установленного файла.
const ASSET: &str = "SkyDesktop.exe";
/// SHA-256 exe в шестнадцатеричном виде, как его пишет `sha256sum`.
const CHECKSUM: &str = "SkyDesktop.exe.sha256";
/// Приставка временных папок: [`crate::install::cleanup_stale`] их убирает.
const STAGE_PREFIX: &str = "SkyDesktop-setup-update-";

/// Релиз новее текущей сборки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Версия без приставки `v`, например `0.2.0`.
    pub version: String,
    exe_url: String,
    checksum_url: String,
}

/// Версия этой сборки.
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Последний релиз, если он новее этой сборки.
pub fn latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body: serde_json::Value = client(Duration::from_secs(20))?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|err| format!("GitHub не ответил: {err}"))?
        .json()
        .map_err(|err| format!("непонятный ответ GitHub: {err}"))?;

    let tag = body["tag_name"].as_str().unwrap_or_default();
    let version = tag.trim_start_matches('v').to_string();
    if !is_newer(&version, current()) {
        return Ok(None);
    }

    let asset_url = |name: &str| {
        body["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|asset| asset["name"].as_str() == Some(name))
            .and_then(|asset| asset["browser_download_url"].as_str())
            .filter(|url| is_release_url(url))
            .map(str::to_string)
    };
    match (asset_url(ASSET), asset_url(CHECKSUM)) {
        (Some(exe_url), Some(checksum_url)) => Ok(Some(Release {
            version,
            exe_url,
            checksum_url,
        })),
        _ => Err(format!("в релизе {tag} нет {ASSET} или {CHECKSUM}")),
    }
}

/// Скачивает exe релиза и сверяет его контрольную сумму.
///
/// Файл кладётся в пустую временную папку под именем установленного exe:
/// ядро копирует при обновлении именно его.
pub fn download(release: &Release) -> Result<PathBuf, String> {
    let client = client(Duration::from_secs(600))?;
    let fetch = |url: &str| {
        client
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|err| format!("не удалось скачать обновление: {err}"))
    };

    let expected = String::from_utf8_lossy(&fetch(&release.checksum_url)?)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let exe = fetch(&release.exe_url)?;
    let actual = hex(&Sha256::digest(&exe));
    if expected.len() != 64 || actual != expected {
        return Err("скачанный файл не совпал с контрольной суммой — обновление отменено".into());
    }

    let dir = std::env::temp_dir().join(format!("{STAGE_PREFIX}{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|err| format!("нет временной папки: {err}"))?;
    let path = dir.join(ASSET);
    std::fs::write(&path, &exe).map_err(|err| format!("не удалось сохранить обновление: {err}"))?;
    tracing::info!("обновление {} скачано: {}", release.version, path.display());
    Ok(path)
}

/// Ставит скачанный exe. Установленное приложение обновляет ядро, переносное
/// ставится обычной установкой — уже новой версией.
#[cfg(windows)]
pub fn install(exe: &std::path::Path) -> Result<(), String> {
    if crate::install::is_installed() {
        librustdesk::platform::update_to(&exe.to_string_lossy())
            .map_err(|err| format!("обновление не запустилось: {err}"))
    } else {
        crate::install::start_from(exe)
    }
}

#[cfg(not(windows))]
pub fn install(_exe: &std::path::Path) -> Result<(), String> {
    Err("обновление поддерживается только в Windows".into())
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        // Без User-Agent API GitHub отвечает 403.
        .user_agent(concat!("SkyDesktop/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .build()
        .map_err(|err| format!("нет HTTP-клиента: {err}"))
}

/// Файлы берём только из релизов своего репозитория.
fn is_release_url(url: &str) -> bool {
    url.starts_with(&format!("https://github.com/{REPO}/releases/download/"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `1.2.3` → (1, 2, 3). Хвост вроде `-beta` не поддерживаем: такой релиз
/// просто не считается новее.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.10.0", "0.9.9"), "сравнение по числам, а не по строкам");
        assert!(is_newer("1.0.0", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        // Непонятная версия — не обновление.
        assert!(!is_newer("0.3.0-beta", "0.2.0"));
        assert!(!is_newer("", "0.2.0"));
    }

    #[test]
    fn accepts_only_own_release_files() {
        assert!(is_release_url(
            "https://github.com/qdpops/SkyDesktop/releases/download/v0.2.0/SkyDesktop.exe"
        ));
        assert!(!is_release_url("https://github.com/other/SkyDesktop/releases/download/v1/SkyDesktop.exe"));
        assert!(!is_release_url("http://github.com/qdpops/SkyDesktop/releases/download/v1/SkyDesktop.exe"));
        assert!(!is_release_url("https://example.com/SkyDesktop.exe"));
    }

    #[test]
    fn hex_matches_sha256sum() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
