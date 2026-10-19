//! Мост к настоящему ядру удалённого доступа (форк RustDesk в `vendor/rustdesk`).
//!
//! Здесь нет симуляции: адрес устройства, пароль, состояние регистрации на
//! сервере-посреднике и входящие подключения берутся из работающего ядра.
//!
//! Роли процессов (их выбирает `core_main`, см. `main.rs`):
//!   * без аргументов — главное окно; в этом же процессе поднят сервер,
//!     который принимает входящие подключения;
//!   * `--cm` — окно менеджера подключений, ядро запускает его само,
//!     когда кто-то подключается к этому компьютеру.
//!
//! Важное ограничение: почти все обращения к ядру внутри себя делают
//! `Runtime::block_on` (IPC к серверному процессу). Из рабочего потока tokio
//! это паника «Cannot start a runtime from within a runtime», поэтому всё
//! общение с ядром вынесено в отдельный обычный поток — [`start`].

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use skydesktop_bridge::{
    ApproveMode, CoreEvent, DeviceStateDto, DeviceStatus, DirectoryStateDto, IncomingClientDto,
    PasswordKind,
};
use tokio::sync::broadcast;

use librustdesk::ui_cm_interface::{Client, ConnectionManager, InvokeUiCM};
use librustdesk::ui_interface;

/// Как часто поток ядра сверяет состояние устройства.
const POLL_INTERVAL: Duration = Duration::from_millis(1200);

/// Порт прямого доступа. В интерфейсе не настраивается: подключаются
/// просто по IP, а порт ядро подставляет само.
const DIRECT_PORT: &str = "21118";

/// Как часто опрашивается каталог нод.
const DIRECTORY_INTERVAL: Duration = Duration::from_secs(60);

/// Как часто спрашивать, кто из знакомых устройств в сети.
const ONLINE_INTERVAL: Duration = Duration::from_secs(45);


/// Ключ настройки ядра со списком нод из каталога.
const POOL_KEY: &str = hbb_common::config::keys::OPTION_SKYDESKTOP_RENDEZVOUS_POOL;

/// Адрес каталога нод. Вшивается в сборку и в интерфейсе не меняется:
/// задайте `SKYDESKTOP_DIRECTORY` перед компиляцией. Пусто — каталог не
/// используется, и ядро работает с тем сервером, что записан в его настройках.
const DIRECTORY_URL: &str = match option_env!("SKYDESKTOP_DIRECTORY") {
    Some(url) => url,
    None => "",
};

/// Шина событий ядра: публикуется в каждый подключённый UI-вебсокет.
#[derive(Clone)]
pub struct EventBus(broadcast::Sender<CoreEvent>);

impl EventBus {
    pub fn new() -> Self {
        // Ёмкость с запасом: события мелкие, а UI может ненадолго отстать.
        let (tx, _) = broadcast::channel(256);
        Self(tx)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.0.subscribe()
    }

    pub fn publish(&self, event: CoreEvent) {
        // Ошибка означает лишь «нет подписчиков» — это норма.
        let _ = self.0.send(event);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Запрос к ядру от интерфейса.
#[derive(Debug, Clone)]
pub enum Request {
    /// Открыть исходящий сеанс отдельным окном.
    Connect {
        id: String,
        password: String,
        /// Параметры качества из панели, см. `viewer::QualitySpec`.
        quality: String,
    },
    /// Разрешить входящее подключение.
    Authorize(i32),
    /// Отклонить запрос или разорвать установленное подключение.
    CloseConnection(i32),
    /// Задать постоянный пароль доступа к этому компьютеру.
    SetPassword(String),
    /// Сгенерировать новый одноразовый пароль.
    RotatePassword,
    /// Выбрать, как разрешается доступ к этому компьютеру.
    SetApproveMode(ApproveMode),
    /// За какими адресами следить: панель показывает, кто из них в сети.
    WatchPeers(Vec<String>),
    /// Запрашивать ли права администратора при следующем запуске.
    SetElevateOnStart(bool),
    /// Установить приложение на этот компьютер.
    Install,
    /// Скачать и поставить новую версию с GitHub.
    Update,
    /// Проверить обновления сейчас, не дожидаясь фоновой проверки.
    CheckUpdate,
}

/// Что приходит в поток ядра: запрос от интерфейса либо ответ каталога.
enum Message {
    Ui(Request),
    Directory(Result<crate::directory::Document, String>),
    /// Ответ сервера связи: кто из отслеживаемых адресов в сети.
    Online(Vec<String>, Vec<String>),
}

/// Ручка к потоку ядра.
#[derive(Clone)]
pub struct Engine {
    requests: Sender<Message>,
}

impl Engine {
    /// Отправляет запрос ядру. Если поток ядра остановлен, запрос теряется.
    pub fn send(&self, request: Request) {
        if let Err(err) = self.requests.send(Message::Ui(request)) {
            tracing::error!("поток ядра недоступен: {err}");
        }
    }
}

/// Запускает поток, который общается с ядром и публикует его состояние.
pub fn start(bus: EventBus) -> Engine {
    let (tx, rx) = mpsc::channel::<Message>();
    let fetch_tx = tx.clone();

    spawn_update_checker();

    std::thread::Builder::new()
        .name("skydesktop-engine".to_string())
        .spawn(move || {
            // Синхронизация состояния с серверным процессом ядра.
            ui_interface::start_option_status_sync();

            let mut directory = load_directory();
            apply_defaults();
            adopt_builtin_directory();
            claim_permanent_id(&directory);
            // Первый опрос каталога — сразу после старта.
            let mut next_fetch = Some(Instant::now());
            let mut fetching = false;
            let mut last: Option<DeviceStateDto> = None;

            // Присутствие знакомых устройств: список задаёт панель.
            let mut watched: Vec<String> = Vec::new();
            let mut next_online: Option<Instant> = None;
            let mut asking = false;

            loop {
                match rx.recv_timeout(POLL_INTERVAL) {
                    Ok(Message::Ui(Request::WatchPeers(ids))) => {
                        // Спрашивать можно только про адреса устройств:
                        // подключения по IP сервер связи не знает.
                        let ids: Vec<String> = ids
                            .into_iter()
                            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
                            .collect();
                        if ids != watched {
                            watched = ids;
                            next_online = Some(Instant::now());
                        }
                    }
                    Ok(Message::Ui(Request::Install)) => {
                        // Установщик ждёт ответа в окне UAC сколько угодно
                        // долго, поэтому — в своём потоке.
                        if !INSTALLING.swap(true, Ordering::Relaxed) {
                            let bus = bus.clone();
                            std::thread::spawn(move || {
                                let result = crate::install::start();
                                INSTALLING.store(false, Ordering::Relaxed);
                                if let Err(text) = result {
                                    tracing::warn!("установка: {text}");
                                    bus.publish(CoreEvent::Notice {
                                        level: "error".to_string(),
                                        text,
                                    });
                                }
                            });
                        }
                        last = None;
                    }
                    Ok(Message::Ui(Request::CheckUpdate)) => {
                        if !CHECKING.swap(true, Ordering::Relaxed) {
                            let bus = bus.clone();
                            std::thread::spawn(move || {
                                let text = match check_update() {
                                    Ok(Some(version)) => format!("Доступна версия {version}"),
                                    Ok(None) => format!(
                                        "Установлена последняя версия {}",
                                        crate::update::current()
                                    ),
                                    Err(err) => format!("Не удалось проверить обновления: {err}"),
                                };
                                CHECKING.store(false, Ordering::Relaxed);
                                bus.publish(CoreEvent::Notice {
                                    level: "info".to_string(),
                                    text,
                                });
                            });
                        }
                        last = None;
                    }
                    Ok(Message::Ui(Request::Update)) => {
                        let release = UPDATE.lock().unwrap().clone();
                        if let Some(release) = release {
                            if !UPDATING.swap(true, Ordering::Relaxed) {
                                let bus = bus.clone();
                                std::thread::spawn(move || {
                                    let result = crate::update::download(&release)
                                        .and_then(|exe| crate::update::install(&exe));
                                    UPDATING.store(false, Ordering::Relaxed);
                                    if let Err(text) = result {
                                        tracing::warn!("обновление: {text}");
                                        bus.publish(CoreEvent::Notice {
                                            level: "error".to_string(),
                                            text,
                                        });
                                    }
                                });
                            }
                        }
                        last = None;
                    }
                    Ok(Message::Ui(request)) => {
                        match handle(request) {
                            Outcome::Event(event) => bus.publish(event),
                            Outcome::Nothing => {}
                        }
                        // Команда могла изменить пароль или список нод.
                        last = None;
                    }
                    Ok(Message::Online(online, offline)) => {
                        asking = false;
                        next_online = Some(Instant::now() + ONLINE_INTERVAL);
                        bus.publish(CoreEvent::PeerStates { online, offline });
                    }
                    Ok(Message::Directory(result)) => {
                        fetching = false;
                        next_fetch = Some(Instant::now() + DIRECTORY_INTERVAL);
                        if let Some(event) = apply_directory(result, &mut directory) {
                            bus.publish(event);
                        }
                        last = None;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }

                // Опрос каталога уходит в отдельный поток: сетевой обмен не должен
                // задерживать выдачу состояния устройства.
                if !fetching && !directory.url.is_empty() {
                    if next_fetch.map(|at| Instant::now() >= at).unwrap_or(false) {
                        fetching = true;
                        next_fetch = None;
                        let url = directory.url.clone();
                        let tx = fetch_tx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(Message::Directory(crate::directory::fetch(&url)));
                        });
                    }
                }

                // Присутствие спрашиваем у сервера связи отдельным потоком:
                // ответ занимает до трёх секунд.
                if !asking && !watched.is_empty() {
                    if next_online.map(|at| Instant::now() >= at).unwrap_or(false) {
                        asking = true;
                        next_online = None;
                        let ids = watched.clone();
                        let tx = fetch_tx.clone();
                        std::thread::spawn(move || {
                            let answer = query_online(ids);
                            let _ = tx.send(Message::Online(answer.0, answer.1));
                        });
                    }
                }

                // Проверяется каждый цикл, а не только на старте: первая попытка
                // может уйти раньше, чем поднимется IPC серверного процесса.
                enforce_always_on();

                let state = device_state(&directory);
                if last.as_ref().map(key) != Some(key(&state)) {
                    last = Some(state.clone());
                    bus.publish(CoreEvent::DeviceState { state });
                }
            }
        })
        .expect("не удалось запустить поток ядра");

    Engine { requests: tx }
}

/// Спрашивает сервер связи, кто из адресов сейчас в сети.
fn query_online(ids: Vec<String>) -> (Vec<String>, Vec<String>) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::warn!("не удалось создать рантайм для опроса присутствия: {err}");
            return (Vec::new(), Vec::new());
        }
    };

    let answer = std::sync::Arc::new(std::sync::Mutex::new((Vec::new(), Vec::new())));
    let sink = answer.clone();
    runtime.block_on(async move {
        librustdesk::client::peer_online::query_online_states(ids, move |online, offline| {
            *sink.lock().unwrap() = (online, offline);
        })
        .await;
    });

    let answer = answer.lock().unwrap();
    answer.clone()
}

/// Результат обработки запроса интерфейса.
enum Outcome {
    Event(CoreEvent),
    Nothing,
}

/// Каталог и последний известный пул — как они сохранены в настройках ядра.
fn load_directory() -> DirectoryStateDto {
    DirectoryStateDto {
        url: DIRECTORY_URL.to_string(),
        nodes: split_list(&ui_interface::get_option(POOL_KEY)),
        relays: Vec::new(),
        error: String::new(),
    }
}

/// Значения по умолчанию для первого запуска.
///
/// В отличие от [`enforce_always_on`] это именно умолчание: пользователь
/// вправе поменять его в настройках, и повторно мы его не навязываем.
fn apply_defaults() {
    // Обычному пользователю пароль не нужен: подключение подтверждают в окне.
    if ui_interface::get_option("approve-mode").is_empty() {
        tracing::info!("первый запуск: доступ по подтверждению");
        set_option("approve-mode", "click".to_string());
    }
}

/// Приём подключений и прямой доступ по IP включены всегда.
///
/// Это не настройки, а условия работы приложения. Пишем только при
/// расхождении, поэтому проверка дешёвая и переживает недоступность IPC.
fn enforce_always_on() {
    for (key, value) in [
        // Ядро читает признак наоборот: "Y" означает «служба остановлена».
        ("stop-service", ""),
        ("direct-server", "Y"),
        ("direct-access-port", DIRECT_PORT),
    ] {
        if ui_interface::get_option(key) != value {
            tracing::info!("включаем обязательную настройку {key}");
            set_option(key, value.to_string());
        }
    }
}

/// Со вшитым каталогом источник истины — он.
///
/// Адреса из прежних настроек только мешают: сервер попал бы в пул затравкой,
/// а заданный вручную ретранслятор перебил бы тот, что назначает сам пул.
fn adopt_builtin_directory() {
    if DIRECTORY_URL.is_empty() {
        return;
    }
    for key in ["custom-rendezvous-server", "relay-server", "api-server"] {
        if !ui_interface::get_option(key).is_empty() {
            tracing::info!("каталог вшит в сборку — убираем {key} из настроек");
            set_option(key, String::new());
        }
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_owned())
        .collect()
}

/// Переносит ответ каталога в настройки ядра.
///
/// Пишем только при реальном изменении: любая запись настроек перезапускает
/// связь с сервером-посредником (`CheckIfRestart` в ядре).
fn apply_directory(
    result: Result<crate::directory::Document, String>,
    directory: &mut DirectoryStateDto,
) -> Option<CoreEvent> {
    let document = match result {
        Ok(document) => document,
        Err(error) => {
            let changed = directory.error != error;
            tracing::warn!("каталог нод: {error}");
            directory.error = error.clone();
            return changed.then(|| CoreEvent::Notice {
                level: "error".to_string(),
                text: format!("Каталог нод: {error}"),
            });
        }
    };

    directory.error = String::new();
    directory.relays = document.relay.clone();

    let pool = document.rendezvous.join(",");
    let mut changed = false;
    if ui_interface::get_option(POOL_KEY) != pool {
        set_option(POOL_KEY, pool);
        changed = true;
    }
    if !document.key.is_empty() && ui_interface::get_option("key") != document.key {
        set_option("key", document.key.clone());
        changed = true;
    }
    if !changed {
        return None;
    }

    let count = document.rendezvous.len();
    directory.nodes = document.rendezvous;
    Some(CoreEvent::Notice {
        level: "info".to_string(),
        text: format!("Каталог нод обновлён: серверов в пуле — {count}"),
    })
}

/// Просит менеджер подключений разорвать сеанс.
fn request_disconnect(control: &str, conn_id: i32) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| err.to_string())?;

    client
        .post(format!("{control}/disconnect"))
        .json(&serde_json::json!({ "connId": conn_id }))
        .send()
        .and_then(|response| response.error_for_status())
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Как значение режима называется в настройках ядра.
fn approve_mode_key(mode: ApproveMode) -> &'static str {
    match mode {
        ApproveMode::Password => "password",
        ApproveMode::Click => "click",
        ApproveMode::Both => "both",
    }
}

/// Записывает одну настройку ядра.
///
/// Именно `set_option`, а не `set_options`: последняя заменяет весь набор
/// настроек тем, что ей передали, и частичной картой стирает всё остальное.
fn set_option(key: &str, value: String) {
    ui_interface::set_option(key.to_string(), value);
}

/// Ключ сравнения состояний — в UI уходят только реальные изменения.
fn key(state: &DeviceStateDto) -> (String, String, u8, u8, Option<i32>, String, (bool, bool)) {
    (
        state.id.clone(),
        state.password.clone(),
        state.status as u8,
        state.approve_mode as u8,
        state.latency_ms,
        format!(
            "{}|{}",
            state.directory.nodes.join(","),
            state.directory.error
        ),
        (state.installed, state.installing),
    )
}

fn handle(request: Request) -> Outcome {
    match request {
        Request::Connect {
            id,
            password,
            quality,
        } => {
            // Каждый сеанс — своё окно и свой процесс: так же, как это делает
            // ядро, и так сбой сеанса не роняет панель управления.
            let args = vec![
                "--connect".to_string(),
                id.clone(),
                password,
                "--quality".to_string(),
                quality,
            ];
            match librustdesk::common::run_me(args) {
                Ok(_) => Outcome::Event(CoreEvent::SessionStarted {
                    name: format!("Устройство {id}"),
                    address: id,
                }),
                Err(err) => Outcome::Event(CoreEvent::Notice {
                    level: "error".to_string(),
                    text: format!("Не удалось открыть сеанс: {err}"),
                }),
            }
        }
        Request::Authorize(conn_id) => {
            librustdesk::ui_cm_interface::authorize(conn_id);
            Outcome::Nothing
        }
        Request::CloseConnection(conn_id) => {
            // В процессе панели реестра подключений нет — команду принимает
            // менеджер, который их и держит.
            let control = crate::control::get();
            tracing::info!("разрыв {conn_id}: менеджер на «{control}»");
            if control.is_empty() {
                librustdesk::ui_cm_interface::close(conn_id);
            } else if let Err(err) = request_disconnect(&control, conn_id) {
                return Outcome::Event(CoreEvent::Notice {
                    level: "error".to_string(),
                    text: format!("Не удалось прервать доступ: {err}"),
                });
            }
            Outcome::Nothing
        }
        Request::SetPassword(password) => {
            if ui_interface::set_permanent_password_with_result(password) {
                Outcome::Event(CoreEvent::Notice {
                    level: "info".to_string(),
                    text: "Постоянный пароль сохранён".to_string(),
                })
            } else {
                Outcome::Event(CoreEvent::Notice {
                    level: "error".to_string(),
                    text: "Не удалось сохранить пароль".to_string(),
                })
            }
        }
        Request::RotatePassword => {
            ui_interface::update_temporary_password();
            Outcome::Nothing
        }
        // Обрабатываются в самом цикле: там живёт список отслеживаемых
        // адресов и шина для ответа установщика.
        Request::WatchPeers(_) | Request::Install | Request::Update | Request::CheckUpdate => {
            Outcome::Nothing
        }
        Request::SetElevateOnStart(on) => {
            // Пишем именно «N», а не пустую строку: пустое значение ядро
            // считает отсутствием настройки, а отсутствие у нас означает
            // «включено».
            set_option(ELEVATE_OPTION, if on { "Y" } else { "N" }.to_string());
            Outcome::Event(CoreEvent::Notice {
                level: "info".to_string(),
                text: if on {
                    "Права администратора запросятся при следующем запуске".to_string()
                } else {
                    "Приложение будет запускаться с обычными правами".to_string()
                },
            })
        }
        Request::SetApproveMode(mode) => {
            set_option("approve-mode", approve_mode_key(mode).to_string());
            Outcome::Event(CoreEvent::Notice {
                level: "info".to_string(),
                text: match mode {
                    ApproveMode::Password => "Доступ — по паролю".to_string(),
                    ApproveMode::Click => "Доступ — по подтверждению на этом компьютере".to_string(),
                    ApproveMode::Both => "Доступ — по паролю, иначе по подтверждению".to_string(),
                },
            })
        }
    }
}

/// Текущее состояние устройства для блока «Это устройство».
///
/// Вызывать только из потока ядра — внутри идут блокирующие IPC-запросы.
/// Закрепляет за этим компьютером адрес, выданный каталогом.
///
/// Ядро само придумывает адрес случайно и меняет его, как только сервер
/// отвечает `UUID_MISMATCH`. Записанный у пользователя адрес после этого
/// перестаёт работать, поэтому адрес выдаёт каталог — по отпечатку железа,
/// один и тот же при каждом запуске.
fn claim_permanent_id(directory: &DirectoryStateDto) {
    if directory.url.is_empty() {
        return;
    }

    let machine = machine_fingerprint();
    if machine.is_empty() {
        tracing::warn!("отпечаток машины недоступен: адрес останется случайным");
        return;
    }

    match crate::directory::claim_id(&directory.url, &machine) {
        Ok(id) => {
            // Ядру этот адрес менять нельзя: он закреплён за железом.
            set_option(
                hbb_common::config::keys::OPTION_SKYDESKTOP_FIXED_ID,
                "Y".to_string(),
            );

            let current = hbb_common::config::Config::get_id();
            if current == id {
                tracing::info!("адрес {id} подтверждён каталогом");
                return;
            }
            tracing::info!("каталог закрепил адрес {id} (был {current})");
            hbb_common::config::Config::set_id(&id);
            // Новый адрес сервер связи ещё не видел — пусть ядро
            // зарегистрирует его заново.
            hbb_common::config::Config::set_key_confirmed(false);
        }
        Err(err) => tracing::warn!("каталог не выдал адрес: {err}"),
    }
}

/// Отпечаток железа: тот же, что ядро отправляет серверу связи.
fn machine_fingerprint() -> String {
    String::from_utf8_lossy(&hbb_common::get_uuid()).trim().to_string()
}

fn device_state(directory: &DirectoryStateDto) -> DeviceStateDto {
    // Локальная проверка: вариант через IPC на каждый опрос засыпает лог
    // предупреждениями синхронизации хранилища паролей.
    let permanent = hbb_common::config::Config::has_permanent_password();
    // `status_num` says only that the server answered: ядро обновляет
    // задержку даже на отказ регистрации (UUID_MISMATCH, NOT_DEPLOYED).
    // «Готов» означает, что сервер закрепил за нами адрес, а это `key_confirmed`.
    // Опрос присутствия для этой проверки не годится: при сбое связи ядро
    // возвращает все адреса как офлайн, и свой собственный мы бы объявили
    // незакреплённым на ровном месте. `key_confirmed` ставит сам сервер.
    let connection = ui_interface::get_connect_status();
    let status = match (connection.status_num, connection.key_confirmed) {
        (1, true) => DeviceStatus::Ready,
        (1, false) => DeviceStatus::Unconfirmed,
        (0, _) => DeviceStatus::Offline,
        _ => DeviceStatus::Connecting,
    };

    DeviceStateDto {
        id: librustdesk::ipc::get_id(),
        // Показываем одноразовый пароль: постоянный ядро хранит хешем и
        // обратно не отдаёт, так что показать его невозможно в принципе.
        password: ui_interface::temporary_password(),
        // Вид означает «задан ли также постоянный пароль».
        password_kind: if permanent {
            PasswordKind::Permanent
        } else {
            PasswordKind::Temporary
        },
        status,
        approve_mode: match ui_interface::get_option("approve-mode").as_str() {
            "password" => ApproveMode::Password,
            "click" => ApproveMode::Click,
            // Пустое значение ядро трактует как «оба».
            _ => ApproveMode::Both,
        },
        // Ядро меряет задержку при перерегистрации и хранит её в микросекундах.
        latency_ms: match hbb_common::config::get_online_state() {
            micros if micros > 0 => Some(((micros + 500) / 1000) as i32),
            _ => None,
        },
        directory: directory.clone(),
        simulated: false,
        elevate_on_start: elevate_on_start(),
        elevated: is_elevated(),
        installed: crate::install::is_installed(),
        installing: INSTALLING.load(Ordering::Relaxed),
        update_version: UPDATE.lock().unwrap().as_ref().map(|r| r.version.clone()),
        updating: UPDATING.load(Ordering::Relaxed),
        checking_update: CHECKING.load(Ordering::Relaxed),
    }
}

/// Установка идёт: второе нажатие не должно запускать её ещё раз.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Как часто спрашивать GitHub о новой версии.
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Найденный релиз новее этой сборки.
static UPDATE: Mutex<Option<crate::update::Release>> = Mutex::new(None);
static UPDATING: AtomicBool = AtomicBool::new(false);
/// Идёт проверка по кнопке в настройках.
static CHECKING: AtomicBool = AtomicBool::new(false);

/// Спрашивает GitHub и запоминает результат; возвращает новую версию, если она есть.
fn check_update() -> Result<Option<String>, String> {
    let release = crate::update::latest()?;
    let version = release.as_ref().map(|r| r.version.clone());
    if let Some(version) = &version {
        tracing::info!("доступна версия {version}");
    }
    *UPDATE.lock().unwrap() = release;
    Ok(version)
}

/// Фоновая проверка обновлений: при старте и затем раз в [`UPDATE_INTERVAL`].
/// Панель увидит результат в состоянии устройства при следующем опросе.
fn spawn_update_checker() {
    let _ = std::thread::Builder::new()
        .name("skydesktop-update".to_string())
        .spawn(|| loop {
            // Без сети или при сбое GitHub просто попробуем позже.
            if let Err(err) = check_update() {
                tracing::warn!("проверка обновлений: {err}");
            }
            std::thread::sleep(UPDATE_INTERVAL);
        });
}

/// Настройка «запрашивать права администратора при запуске».
pub const ELEVATE_OPTION: &str = "skydesktop-elevate";

/// Запрашивать ли права администратора при запуске.
///
/// По умолчанию — да: без них на удалённой стороне молча не проходят
/// действия в окнах, запущенных от администратора, и понять причину со
/// стороны невозможно. Выключить можно в настройках.
pub fn elevate_on_start() -> bool {
    hbb_common::config::Config::get_option(ELEVATE_OPTION) != "N"
}

/// Работает ли текущий процесс с правами администратора.
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        librustdesk::platform::is_elevated(None).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Текущие подключения к этому компьютеру.
///
/// Шина доставляет события только тем, кто уже подписан, а интерфейс окна
/// менеджера подключается к мосту через секунду после старта процесса —
/// к этому моменту запрос успевает уйти в пустоту. Поэтому список хранится
/// отдельно и выдаётся каждому новому подключению целиком.
pub type Clients = Arc<Mutex<Vec<IncomingClientDto>>>;

/// Приёмник событий менеджера подключений: перекладывает их в шину UI.
#[derive(Clone)]
pub struct CmHandler {
    bus: EventBus,
    clients: Clients,
    /// Адрес, по которому панель просит разорвать подключение.
    control: String,
}

impl CmHandler {
    fn dto(client: &Client) -> IncomingClientDto {
        IncomingClientDto {
            conn_id: client.id,
            peer_id: client.peer_id.clone(),
            name: client.name.clone(),
            authorized: client.authorized,
            keyboard: client.keyboard,
            clipboard: client.clipboard,
            file: client.file,
        }
    }
}

impl CmHandler {
    /// Панель показывает подключения, но менеджер живёт отдельным процессом,
    /// поэтому список ей пересылается.
    fn publish_clients(&self) {
        let clients = self.clients.lock().unwrap().clone();
        crate::metrics::report_clients(self.control.clone(), clients);
    }
}

impl InvokeUiCM for CmHandler {
    fn add_connection(&self, client: &Client) {
        let dto = Self::dto(client);
        tracing::info!(
            "менеджер: запрос {} от {} ({}), подтверждён: {}",
            dto.conn_id,
            dto.name,
            dto.peer_id,
            dto.authorized
        );
        {
            let mut clients = self.clients.lock().unwrap();
            clients.retain(|c| c.conn_id != dto.conn_id);
            clients.push(dto.clone());
        }
        self.bus.publish(CoreEvent::IncomingConnection { client: dto });
        self.publish_clients();
    }

    fn remove_connection(&self, id: i32, _close: bool) {
        tracing::info!("менеджер: подключение {id} завершено");
        self.clients.lock().unwrap().retain(|c| c.conn_id != id);
        self.bus.publish(CoreEvent::ConnectionClosed { conn_id: id });
        self.publish_clients();
    }

    fn new_message(&self, _id: i32, text: String) {
        self.bus.publish(CoreEvent::Notice {
            level: "info".to_string(),
            text,
        });
    }

    fn change_theme(&self, _dark: String) {}

    fn change_language(&self) {}

    fn show_elevation(&self, _show: bool) {}

    fn update_voice_call_state(&self, client: &Client) {
        self.add_connection(client);
    }

    fn file_transfer_log(&self, _action: &str, _log: &str) {}
}

/// Поднимает менеджер входящих подключений и связывает его с шиной событий.
///
/// Вызывается только в режиме `--cm`: именно этот процесс ядро запускает,
/// когда кто-то подключается к компьютеру.
pub fn start_connection_manager(bus: EventBus, control: String) -> Clients {
    let clients: Clients = Default::default();
    let cm = ConnectionManager {
        ui_handler: CmHandler {
            bus,
            clients: clients.clone(),
            control,
        },
    };
    std::thread::Builder::new()
        .name("skydesktop-cm".to_string())
        .spawn(move || librustdesk::ui_cm_interface::start_ipc(cm))
        .expect("не удалось запустить менеджер подключений");
    clients
}
