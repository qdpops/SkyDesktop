//! SkyDesktop — оконное приложение удалённого рабочего стола.
//!
//! Процесс один и тот же, роль выбирается аргументами — так же, как в ядре:
//!   * без аргументов — главное окно (панель управления). В этом же процессе
//!     ядро поднимает сервер, который принимает входящие подключения;
//!   * `--cm` — окно «Входящее подключение»; его запускает само ядро,
//!     когда кто-то подключается к этому компьютеру;
//!   * `--connect <адрес> [пароль]` — окно удалённого сеанса;
//!   * прочие служебные режимы (`--server`, `--service`, `--tray`, установка,
//!     смена пароля и т. д.) целиком обрабатывает `core_main` ядра.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod approval;
mod bridge;
mod control;
mod diagnostics;
mod directory;
mod engine;
mod history;
mod install;
mod icons;
mod metrics;
mod paint;
mod panel;
mod server;
mod update;
mod viewer;

fn main() {
    // Сборка без консоли: без этого сбой при старте выглядел бы как
    // мигнувшее и закрывшееся окно, без единого следа.
    diagnostics::install_panic_hook();
    diagnostics::redirect_stderr(role());
    init_tracing();

    // Ядро настраивает `env_logger` из `RUST_LOG`. Без переменной в журнал
    // попадают только ошибки, а разбираться приходится по ходу событий.
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }

    // Своё имя задаётся до core_main: от него зависят папка настроек, имена
    // каналов IPC и системной службы. С именем RustDesk приложение делило бы
    // адрес устройства с установленным RustDesk и трогало его службу.
    *hbb_common::config::APP_NAME.write().unwrap() = "SkyDesktop".to_string();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    match raw.first().map(String::as_str) {
        // Копия-установщик, её запускает кнопка «Установить» в панели.
        Some(install::FLAG) => std::process::exit(install::run_installer(&raw[1..])),
        // Значок в трее из ядра нам не нужен: он с чужими надписями и
        // дублирует панель. Ядро и ярлык автозагрузки запускают его сами —
        // просто тихо выходим.
        Some("--tray") => return,
        _ => {}
    }

    // core_main разбирает служебные режимы и возвращает Some(args) только
    // тогда, когда процессу действительно нужно показать окно.
    let Some(args) = librustdesk::core_main::core_main() else {
        librustdesk::common::global_clean();
        return;
    };

    if args.is_empty() {
        // Права запрашиваем только у панели: именно её процесс принимает
        // входящие подключения, а окна сеанса и плашки наследуют его права.
        if elevate_if_requested() {
            return;
        }
        install::cleanup_stale();
        panel::run();
    } else if args[0] == "--cm" {
        approval::run();
    } else if args[0] == "--connect" && args.len() > 1 {
        let id = args[1].clone();
        let password = args.get(2).cloned().unwrap_or_default();
        // `--quality` — наш собственный флаг, ядру его передавать не нужно.
        let tail = &args[3.min(args.len())..];
        let quality = tail
            .iter()
            .position(|a| a == "--quality")
            .and_then(|i| tail.get(i + 1))
            .cloned()
            .unwrap_or_else(|| "auto".to_string());
        let rest = tail
            .iter()
            .filter(|a| *a != "--quality" && **a != quality)
            .cloned()
            .collect::<Vec<_>>();
        viewer::run(id, password, quality, rest);
    } else {
        eprintln!("SkyDesktop: неизвестный режим запуска {:?}", args);
    }

    librustdesk::common::global_clean();
}

/// Перезапускает приложение с правами администратора, если так настроено.
///
/// Без них Windows не даёт слать ввод в окна, запущенные от администратора
/// (диспетчер задач, установщики), — на удалённой стороне такие действия
/// просто не проходят. Возвращает `true`, когда управление передано
/// новому процессу и этот должен завершиться.
#[cfg(windows)]
fn elevate_if_requested() -> bool {
    // Установленному приложению права не нужны: подключения принимает
    // служба, а она и так работает от имени системы.
    if !engine::elevate_on_start() || install::is_running_installed() {
        return false;
    }
    // Если определить права не удалось, считаем, что они уже есть: иначе
    // повышенная копия запросила бы их снова, и так по кругу.
    if librustdesk::platform::is_elevated(None).unwrap_or(true) {
        return false;
    }

    match librustdesk::platform::elevate("") {
        Ok(true) => {
            tracing::info!("перезапуск с правами администратора");
            true
        }
        // Отказ в UAC — не повод не запускаться: работаем как есть.
        Ok(false) => {
            tracing::warn!("повышение прав отклонено, продолжаем без них");
            false
        }
        Err(err) => {
            tracing::warn!("не удалось запросить права администратора: {err}");
            false
        }
    }
}

#[cfg(not(windows))]
fn elevate_if_requested() -> bool {
    false
}

/// Роль процесса по аргументам — до того, как их разберёт ядро.
///
/// Нужна только для имени файла журнала: у окна сеанса, плашки и панели
/// журналы разные, иначе они писали бы вперемешку в один файл.
fn role() -> &'static str {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--cm") {
        "cm"
    } else if args.iter().any(|a| a == "--connect") {
        "session"
    } else if args.is_empty() {
        "panel"
    } else {
        "core"
    }
}

/// Журнал приложения.
///
/// Отладочная сборка идёт с консолью, релизная — без неё; в обоих случаях
/// подробность задаётся переменной `SKYDESKTOP_LOG`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("SKYDESKTOP_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .finish();

    // Только приёмник tracing: глобальный логгер крейта `log` не трогаем.
    // Его занимает ядро через `env_logger::init_from_env`, а повторная
    // установка роняет процесс на старте.
    let _ = tracing::subscriber::set_global_default(subscriber);
}
