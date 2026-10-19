//! Установка на этот компьютер.
//!
//! Без установки приложение работает с правами того, кто его запустил, и
//! окна UAC, экран входа и блокировки удалённой стороне не видны: Windows
//! показывает их на защищённом рабочем столе, куда пускает только систему.
//! Установленное приложение принимает подключения из службы Windows от имени
//! системы — и видит всё.
//!
//! Саму установку делает ядро (`install_me`): копирует программу в Program
//! Files, регистрирует службу, ярлыки и запись в «Программах и компонентах».
//! Одно но: ядро копирует не файл, а всю папку, где он лежит, — запусти мы
//! его прямо из «Загрузок», в Program Files уехали бы все загрузки. Поэтому
//! exe сначала переносится в пустую временную папку, и установку выполняет
//! уже эта копия.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Режим запуска копии-установщика: `SkyDesktop.exe --skydesktop-install <pid панели>`.
pub const FLAG: &str = "--skydesktop-install";

/// Приставка временных папок с копией установщика.
const STAGE_PREFIX: &str = "SkyDesktop-setup-";

/// Код выхода установщика, когда в окне UAC нажали «Нет».
const EXIT_DECLINED: i32 = 2;

/// Установлено ли приложение на этот компьютер.
pub fn is_installed() -> bool {
    #[cfg(windows)]
    {
        librustdesk::platform::is_installed()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Запущена ли именно установленная копия, а не переносной exe.
pub fn is_running_installed() -> bool {
    #[cfg(windows)]
    {
        librustdesk::platform::is_cur_exe_the_installed()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Запускает установку и ждёт её конца. Вызывается из панели.
///
/// При успехе установщик сам закрывает этот процесс и открывает уже
/// установленную панель, так что `Ok` сюда обычно не доходит.
pub fn start() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| format!("не найден exe: {err}"))?;
    start_from(&exe)
}

/// То же, но ставит не себя, а `source` — например, скачанное обновление.
/// Если установка сорвётся, панель снова откроется из текущего exe.
pub fn start_from(source: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| format!("не найден exe: {err}"))?;
    let stage = std::env::temp_dir().join(format!("{STAGE_PREFIX}{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|err| format!("нет временной папки: {err}"))?;

    // Имя обязано совпадать с тем, под которым ядро ищет установленный exe.
    let staged = stage.join("SkyDesktop.exe");
    std::fs::copy(source, &staged).map_err(|err| format!("не удалось скопировать exe: {err}"))?;

    tracing::info!("установка: запускаем {}", staged.display());
    let status = Command::new(&staged)
        .arg(FLAG)
        .arg(std::process::id().to_string())
        .arg(&exe)
        .status()
        .map_err(|err| format!("установщик не запустился: {err}"))?;

    match status.code() {
        Some(0) => Ok(()),
        Some(EXIT_DECLINED) => Err("Установка отменена в окне Windows".to_string()),
        _ => Err("Установка не удалась — подробности в журнале".to_string()),
    }
}

/// Тело копии-установщика; возвращает код выхода процесса.
///
/// `args` — всё, что идёт после [`FLAG`]: номер процесса панели и путь к
/// исходному exe. Они нужны, чтобы не оставить человека совсем без окна:
/// если установка упала уже после того, как закрыла панель, открываем её снова.
#[cfg(windows)]
pub fn run_installer(args: &[String]) -> i32 {
    let panel_pid = args.first().and_then(|pid| pid.parse::<u32>().ok());
    let original = args.get(1).map(PathBuf::from);

    // `silent = false`: по окончании ядро само откроет установленную панель.
    match librustdesk::platform::install_me("desktopicon startmenu", String::new(), false, false) {
        Ok(()) => {
            tracing::info!("установка завершена");
            0
        }
        Err(err) => {
            let text = format!("{err:?}");
            // ERROR_CANCELLED: в окне UAC отказали, ничего не менялось.
            let declined = text.contains("0x800704C7") || text.contains("1223");
            tracing::error!("установка не удалась: {text}");
            if !declined {
                if let (Some(pid), Some(exe)) = (panel_pid, original) {
                    if !process_alive(pid) {
                        let _ = Command::new(exe).spawn();
                    }
                }
            }
            if declined {
                EXIT_DECLINED
            } else {
                1
            }
        }
    }
}

#[cfg(not(windows))]
pub fn run_installer(_args: &[String]) -> i32 {
    1
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    let mut system = hbb_common::sysinfo::System::new();
    system.refresh_processes();
    system.process(hbb_common::sysinfo::Pid::from(pid as usize)).is_some()
}

/// Убирает временные копии установщика, оставшиеся от прошлых установок.
///
/// Сама копия удалить себя не может — exe занят, пока она работает.
pub fn cleanup_stale() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let current = std::env::current_exe().ok();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_stage = path
            .file_name()
            .and_then(|name| name.to_str())
            .map_or(false, |name| name.starts_with(STAGE_PREFIX));
        if is_stage && !contains(&path, current.as_deref()) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

fn contains(dir: &Path, file: Option<&Path>) -> bool {
    file.map_or(false, |file| file.starts_with(dir))
}
