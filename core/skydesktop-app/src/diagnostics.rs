//! Диагностика запуска.
//!
//! Приложение собирается без консоли, поэтому паника при старте выглядела бы
//! как «окно мигнуло и закрылось». Здесь причина записывается в файл и
//! показывается пользователю окном, а не теряется.

use std::io::Write;
use std::path::PathBuf;

/// Куда писать, если что-то пошло не так до появления интерфейса.
pub fn crash_log_path() -> PathBuf {
    let mut path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    path.push("SkyDesktop");
    let _ = std::fs::create_dir_all(&path);
    path.push("skydesktop-crash.log");
    path
}

/// Куда пишется журнал роли (`panel`, `cm`, `session`).
pub fn log_path(role: &str) -> PathBuf {
    let mut path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    path.push("SkyDesktop");
    path.push("logs");
    let _ = std::fs::create_dir_all(&path);
    path.push(format!("{role}.log"));
    path
}

/// Направляет stderr процесса в файл журнала.
///
/// Приложение собрано без консоли, поэтому stderr уходит в никуда — а туда
/// пишут и ядро (`env_logger`), и наш `tracing`. Без файла пользователь не
/// может показать, что происходило, а разработчик — это прочитать.
#[cfg(windows)]
pub fn redirect_stderr(role: &str) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};

    let path = log_path(role);
    // Журнал не должен расти без предела: перед запуском срезаем разросшийся.
    if std::fs::metadata(&path).map(|m| m.len() > 5 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }

    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    unsafe {
        SetStdHandle(STD_ERROR_HANDLE, file.as_raw_handle() as _);
    }
    // Дескриптор должен пережить функцию: на него смотрит вся программа.
    std::mem::forget(file);
}

#[cfg(not(windows))]
pub fn redirect_stderr(_role: &str) {}

/// Записывает причину падения в файл и показывает её пользователю.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = crash_log_path();
        let text = format!(
            "SkyDesktop {} — сбой при запуске\n{info}\n",
            env!("CARGO_PKG_VERSION")
        );

        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(file, "{text}");
        }

        show_error(
            "SkyDesktop не смог запуститься",
            &format!("{info}\n\nПодробности: {}", path.display()),
        );
        previous(info);
    }));
}

/// Показывает сообщение окном.
#[cfg(windows)]
pub fn show_error(title: &str, text: &str) {
    // MB_OK | MB_ICONERROR | MB_SETFOREGROUND
    message_box(title, text, 0x0000_0000 | 0x0000_0010 | 0x0001_0000);
}

#[cfg(windows)]
fn message_box(title: &str, text: &str, flags: u32) -> i32 {
    use std::os::windows::ffi::OsStrExt;

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            wide(text).as_ptr(),
            wide(title).as_ptr(),
            flags,
        )
    }
}

#[cfg(not(windows))]
pub fn show_error(title: &str, text: &str) {
    eprintln!("{title}: {text}");
}

#[cfg(not(windows))]
pub fn ask_yes_no(_title: &str, _text: &str) -> bool {
    false
}
