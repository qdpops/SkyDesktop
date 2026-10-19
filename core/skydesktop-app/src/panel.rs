//! Панель управления — главное окно приложения.
//!
//! Нарисована своими силами на процессоре: ни WebView, ни драйверов GPU.
//! Слева меню, справа синяя полоса заголовка и карточки на светлом фоне.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use skydesktop_bridge::{CoreEvent, DeviceStateDto, DeviceStatus, IncomingClientDto};
use tiny_skia::{Pixmap, Rect};

use tao::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Event, MouseButton, WindowEvent},
    window::CursorIcon,
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopWindowTarget},
    keyboard::{Key, KeyCode, ModifiersState},
    monitor::MonitorHandle,
    window::{Window, WindowBuilder},
};

use crate::engine::{self, EventBus, Request};
use crate::history::{self, Visit};
use crate::icons;
use crate::paint::{self, palette, Fonts, Hit};

/// Логотип приложения — тот же, что у окна и у exe.
const LOGO: &[u8] = include_bytes!("../../../assets/brand.png");

const SIDEBAR: f32 = 258.0;
const STRIP: f32 = 46.0;
const PAD: f32 = 22.0;
const CARD_RADIUS: f32 = 14.0;
/// Просвет между карточками.
const CARD_GAP: f32 = 16.0;
/// Высота заголовка раздела внутри карточки.
const HEAD_H: f32 = 60.0;
/// Высота строки списка.
const ROW_H: f32 = 56.0;
/// Высота шапки таблицы истории.
const TABLE_HEAD_H: f32 = 40.0;

/// Что показывает панель.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Remote,
    Settings,
}

/// Куда можно нажать.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Zone {
    None,
    Nav(Page),
    Address,
    Connect,
    CopyId,
    Disconnect(i32),
    HistoryConnect(usize),
    HistoryForget(usize),
    Minimize,
    Maximize,
    Close,
    /// Свободное место полосы заголовка: тянет окно.
    Drag,
    /// Способ разрешения доступа в настройках.
    Mode(skydesktop_bridge::ApproveMode),
    PasswordField,
    PasswordReveal,
    PasswordSave,
    PasswordRotate,
    PasswordCopy,
    /// Переключатель «запрашивать права администратора».
    Elevate,
    /// Кнопка «Установить» в боковой панели.
    Install,
    /// Кнопка «Обновить» — когда на GitHub есть версия новее.
    Update,
    /// Пункт всплывающего меню поля ввода.
    Menu(Action),
}

/// Что умеет меню по правой кнопке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Paste,
    Copy,
    Cut,
    Clear,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Action::Paste => "Вставить",
            Action::Copy => "Копировать",
            Action::Cut => "Вырезать",
            Action::Clear => "Очистить",
        }
    }

    /// Сочетание клавиш — подсказка справа в меню.
    fn hint(self) -> &'static str {
        match self {
            Action::Paste => "Ctrl+V",
            Action::Copy => "Ctrl+C",
            Action::Cut => "Ctrl+X",
            Action::Clear => "Esc",
        }
    }
}

/// Открытое меню поля ввода.
#[derive(Debug, Clone, Copy)]
struct Menu {
    /// Левый верхний угол — там, где нажали правой кнопкой.
    x: f32,
    y: f32,
    /// Поле, к которому меню относится.
    target: Focus,
}

/// Где сейчас ввод с клавиатуры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    None,
    Address,
    Password,
}

/// Состояние окна.
struct State {
    page: Page,
    device: DeviceStateDto,
    clients: Vec<IncomingClientDto>,
    history: Vec<Visit>,
    address: String,
    /// Новый постоянный пароль, пока его набирают.
    password: String,
    /// Показывать одноразовый пароль вместо звёздочек.
    reveal: bool,
    /// Куда идёт ввод с клавиатуры.
    focus: Focus,
    /// Меню правой кнопки, пока оно открыто.
    menu: Option<Menu>,
    /// Отсчёт мигания курсора ввода; сбрасывается при вводе.
    caret: Instant,
    /// Кто из знакомых устройств в сети: ключ — адрес без пробелов.
    online: HashMap<String, bool>,
    /// Сдвиг содержимого вверх при прокрутке колесом.
    scroll: f32,
    hover: Zone,
    notice: Option<(String, Instant)>,
}

impl State {
    fn new() -> Self {
        Self {
            page: Page::Remote,
            device: DeviceStateDto {
                id: "—".to_string(),
                password: String::new(),
                password_kind: skydesktop_bridge::PasswordKind::Temporary,
                status: DeviceStatus::Connecting,
                approve_mode: skydesktop_bridge::ApproveMode::Click,
                latency_ms: None,
                directory: Default::default(),
                simulated: false,
                elevate_on_start: false,
                elevated: false,
                // Пока ядро не ответило, кнопку не показываем: у
                // установленного приложения она мелькнула бы зря.
                installed: true,
                installing: false,
                update_version: None,
                updating: false,
            },
            clients: Vec::new(),
            history: history::load(),
            address: String::new(),
            password: String::new(),
            reveal: false,
            focus: Focus::None,
            menu: None,
            caret: Instant::now(),
            online: HashMap::new(),
            scroll: 0.0,
            hover: Zone::None,
            notice: None,
        }
    }

    /// В сети ли устройство: `None` — ещё не спрашивали.
    fn is_online(&self, address: &str) -> Option<bool> {
        self.online.get(&history::normalize(address)).copied()
    }

    /// Список адресов, за присутствием которых стоит следить.
    fn watched(&self) -> Vec<String> {
        self.history
            .iter()
            .map(|visit| history::normalize(&visit.address))
            .collect()
    }

    /// Курсор ввода мигает: так поле не спутать с обычной надписью.
    fn caret_visible(&self) -> bool {
        self.caret.elapsed().as_millis() % 1060 < 620
    }

    /// Сбрасывает мигание, чтобы после ввода курсор был виден сразу.
    fn wake_caret(&mut self) {
        self.caret = Instant::now();
    }

    /// Содержимое поля, в котором стоит курсор.
    fn field(&self, focus: Focus) -> &str {
        match focus {
            Focus::Address => &self.address,
            Focus::Password => &self.password,
            Focus::None => "",
        }
    }

    fn field_mut(&mut self, focus: Focus) -> Option<&mut String> {
        match focus {
            Focus::Address => Some(&mut self.address),
            Focus::Password => Some(&mut self.password),
            Focus::None => None,
        }
    }

    fn notice(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    fn notice_text(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(4))
            .map(|(text, _)| text.as_str())
    }
}

/// Открывает панель и не возвращает управление до её закрытия.
pub fn run() -> ! {
    let event_loop: EventLoop<CoreEvent> = EventLoopBuilder::with_user_event().build();

    let bus = EventBus::new();
    let engine = engine::start(bus.clone());

    // События ядра приходят в цикл окна.
    {
        let proxy = event_loop.create_proxy();
        let mut events = bus.subscribe();
        std::thread::Builder::new()
            .name("skydesktop-panel-events".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("не удалось создать рантайм");
                runtime.block_on(async move {
                    while let Ok(event) = events.recv().await {
                        let _ = proxy.send_event(event);
                    }
                });
            })
            .expect("не удалось запустить приём событий");
    }

    // Локальный сервер нужен только для сообщений от других окон приложения.
    let runtime = tokio::runtime::Runtime::new().expect("не удалось создать рантайм");
    let (listener, addr) = runtime
        .block_on(crate::server::bind_preferring(Some(
            crate::server::PREFERRED_PORT,
        )))
        .expect("не удалось занять локальный порт");
    runtime.spawn(crate::server::serve(
        listener,
        crate::bridge::router(crate::bridge::BridgeState {
            bus,
            clients: Default::default(),
        }),
    ));
    tracing::info!("панель принимает сообщения окон на http://{addr}/");

    let fonts = Fonts::load();
    let logo = Pixmap::decode_png(LOGO).ok();
    let scale = primary_scale(&event_loop);
    let placement = Placement::for_screen(&event_loop, scale);

    let window = Rc::new(
        WindowBuilder::new()
            .with_title("SkyDesktop — удалённый рабочий стол")
            .with_inner_size(placement.size)
            .with_min_inner_size(placement.min_size)
            .with_position(placement.position)
            .with_decorations(false)
            .with_window_icon(crate::viewer::app_icon())
            .build(&event_loop)
            .expect("не удалось создать окно"),
    );

    let context = softbuffer::Context::new(window.clone()).expect("нет графического контекста");
    let mut surface =
        softbuffer::Surface::new(&context, window.clone()).expect("не удалось создать поверхность");

    let mut state = State::new();
    engine.send(Request::WatchPeers(state.watched()));
    let mut cursor = PhysicalPosition::new(-1.0f64, -1.0f64);
    // Ctrl приходит отдельным событием, в самом нажатии клавиши его нет.
    let mut modifiers = ModifiersState::empty();

    event_loop.run(move |event, _target, control_flow| {
        // Пока поле в фокусе или висит сообщение, окно надо будить: курсор
        // мигает, а сообщение должно само погаснуть.
        *control_flow = if state.focus == Focus::None && state.notice.is_none() {
            ControlFlow::Wait
        } else {
            ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(180))
        };

        match event {
            Event::NewEvents(tao::event::StartCause::ResumeTimeReached { .. }) => {
                window.request_redraw()
            }

            Event::UserEvent(event) => {
                apply(&mut state, event);
                window.request_redraw();
            }

            Event::RedrawRequested(_) => {
                draw(&window, &mut surface, fonts.as_ref(), logo.as_ref(), &state, scale)
            }

            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::Resized(_) => window.request_redraw(),

                WindowEvent::CursorMoved { position, .. } => {
                    cursor = position;
                    let zone = layout(&window, &state, scale).zone_at(position, scale);
                    if zone != state.hover {
                        // Форма курсора сразу показывает, куда можно печатать,
                        // а куда — нажимать.
                        window.set_cursor_icon(cursor_for(&zone));
                        state.hover = zone;
                        window.request_redraw();
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    if state.hover != Zone::None {
                        state.hover = Zone::None;
                        window.request_redraw();
                    }
                }

                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    let zone = layout(&window, &state, scale).zone_at(cursor, scale);
                    click(&mut state, &window, &engine, zone, control_flow);
                    window.request_redraw();
                }

                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Right,
                    ..
                } => {
                    let zone = layout(&window, &state, scale).zone_at(cursor, scale);
                    open_menu(&mut state, zone, cursor, scale);
                    window.request_redraw();
                }

                WindowEvent::MouseWheel { delta, .. } => {
                    let step = match delta {
                        tao::event::MouseScrollDelta::LineDelta(_, lines) => lines * 48.0,
                        tao::event::MouseScrollDelta::PixelDelta(position) => position.y as f32,
                        _ => 0.0,
                    };
                    let layout = layout(&window, &state, scale);
                    let scrolled = (state.scroll - step).clamp(0.0, layout.max_scroll());
                    if (scrolled - state.scroll).abs() > f32::EPSILON {
                        state.scroll = scrolled;
                        // Зона под курсором изменилась вместе с содержимым.
                        state.hover = layout.zone_at(cursor, scale);
                        window.request_redraw();
                    }
                }

                WindowEvent::ModifiersChanged(changed) => modifiers = changed,

                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state == ElementState::Pressed
                        && key(&mut state, &engine, &event, modifiers)
                    {
                        window.request_redraw();
                    }
                }

                _ => {}
            },

            _ => {}
        }
    })
}

/// Переносит событие ядра в состояние окна.
fn apply(state: &mut State, event: CoreEvent) {
    match event {
        // Одноразовый пароль приходит здесь же; постоянный ядро не отдаёт,
        // поэтому поле ввода трогать нечем.
        CoreEvent::DeviceState { state: device } => state.device = device,
        CoreEvent::IncomingConnection { client } => {
            state.clients.retain(|c| c.conn_id != client.conn_id);
            state.clients.push(client);
        }
        CoreEvent::ConnectionClosed { conn_id } => {
            state.clients.retain(|c| c.conn_id != conn_id);
        }
        CoreEvent::PeerName { address, name } => {
            history::set_name(&mut state.history, &address, &name);
        }
        CoreEvent::PeerStates { online, offline } => {
            for id in online {
                state.online.insert(history::normalize(&id), true);
            }
            for id in offline {
                state.online.insert(history::normalize(&id), false);
            }
        }
        CoreEvent::PasswordRequired { address } => {
            state.notice(format!("Для {address} нужен пароль"));
        }
        CoreEvent::Notice { text, .. } => state.notice(text),
        CoreEvent::SessionStarted { .. }
        | CoreEvent::SessionClosed
        | CoreEvent::SessionMetrics { .. } => {}
    }
}

fn click(
    state: &mut State,
    window: &Window,
    engine: &engine::Engine,
    zone: Zone,
    control_flow: &mut ControlFlow,
) {
    // Открытое меню перехватывает нажатие: либо пункт, либо просто закрыть.
    if let Some(menu) = state.menu.take() {
        if let Zone::Menu(action) = zone {
            run_action(state, menu.target, action);
        }
        return;
    }

    state.focus = match zone {
        Zone::Address => Focus::Address,
        Zone::PasswordField => Focus::Password,
        _ => Focus::None,
    };
    state.wake_caret();

    match zone {
        Zone::Nav(page) => {
            state.page = page;
            state.scroll = 0.0;
        }
        Zone::Connect => connect(state, engine, None),
        Zone::CopyId => {
            let id = state.device.id.clone();
            match copy_to_clipboard(&id) {
                Ok(()) => state.notice("Адрес скопирован"),
                Err(err) => state.notice(format!("Не удалось скопировать: {err}")),
            }
        }
        Zone::Disconnect(conn_id) => engine.send(Request::CloseConnection(conn_id)),
        Zone::HistoryConnect(index) => {
            if let Some(visit) = state.history.get(index).cloned() {
                connect(state, engine, Some(visit.address));
            }
        }
        Zone::HistoryForget(index) => {
            if index < state.history.len() {
                state.history.remove(index);
                history::save(&state.history);
                engine.send(Request::WatchPeers(state.watched()));
            }
        }
        Zone::Minimize => window.set_minimized(true),
        Zone::Maximize => window.set_maximized(!window.is_maximized()),
        Zone::Close => *control_flow = ControlFlow::Exit,
        // Свободное место полосы заголовка тянет окно.
        Zone::Drag => {
            let _ = window.drag_window();
        }
        Zone::Mode(mode) => {
            engine.send(Request::SetApproveMode(mode));
            state.device.approve_mode = mode;
        }
        Zone::PasswordSave => save_password(state, engine),
        Zone::PasswordRotate => {
            engine.send(Request::RotatePassword);
            state.notice("Одноразовый пароль обновлён");
        }
        Zone::PasswordCopy => {
            let password = state.device.password.clone();
            if password.is_empty() {
                state.notice("Ядро ещё не выдало пароль");
            } else {
                match copy_to_clipboard(&password) {
                    Ok(()) => state.notice("Одноразовый пароль скопирован"),
                    Err(err) => state.notice(format!("Не удалось скопировать: {err}")),
                }
            }
        }
        Zone::PasswordReveal => state.reveal = !state.reveal,
        Zone::Elevate => {
            let on = !state.device.elevate_on_start;
            engine.send(Request::SetElevateOnStart(on));
            state.device.elevate_on_start = on;
        }
        Zone::Update => {
            if !state.device.updating {
                engine.send(Request::Update);
                state.device.updating = true;
                state.notice("Скачиваем обновление — затем подтвердите его в окне Windows");
            }
        }
        Zone::Install => {
            if !state.device.installing {
                engine.send(Request::Install);
                state.device.installing = true;
                state.notice("Подтвердите установку в окне Windows");
            }
        }
        Zone::None | Zone::Address | Zone::PasswordField | Zone::Menu(_) => {}
    }
}

/// Правая кнопка открывает меню только над полем ввода.
fn open_menu(state: &mut State, zone: Zone, cursor: PhysicalPosition<f64>, scale: f64) {
    let target = match zone {
        Zone::Address => Focus::Address,
        Zone::PasswordField => Focus::Password,
        // Везде остальном правая кнопка просто прячет меню.
        _ => {
            state.menu = None;
            return;
        }
    };

    state.focus = target;
    state.wake_caret();
    state.menu = Some(Menu {
        x: (cursor.x / scale) as f32,
        y: (cursor.y / scale) as f32,
        target,
    });
}

/// Выполняет пункт меню над полем, для которого оно открывалось.
fn run_action(state: &mut State, target: Focus, action: Action) {
    match action {
        Action::Paste => {
            state.focus = target;
            paste(state);
        }
        Action::Copy | Action::Cut => {
            let text = state.field(target).to_string();
            if text.is_empty() {
                state.notice("Поле пустое");
                return;
            }
            match copy_to_clipboard(&text) {
                Ok(()) => {
                    if action == Action::Cut {
                        if let Some(field) = state.field_mut(target) {
                            field.clear();
                        }
                    }
                    state.notice("Скопировано в буфер обмена");
                }
                Err(err) => state.notice(format!("Не удалось скопировать: {err}")),
            }
        }
        Action::Clear => {
            if let Some(field) = state.field_mut(target) {
                field.clear();
            }
        }
    }
}

/// Какой курсор показывать над этим местом окна.
fn cursor_for(zone: &Zone) -> CursorIcon {
    match zone {
        // Поля ввода — текстовый курсор: видно, что сюда можно печатать.
        Zone::Address | Zone::PasswordField => CursorIcon::Text,
        Zone::None | Zone::Drag => CursorIcon::Default,
        _ => CursorIcon::Hand,
    }
}

fn connect(state: &mut State, engine: &engine::Engine, address: Option<String>) {
    let raw = address.unwrap_or_else(|| state.address.clone());
    let address = clean_address(&raw);
    if address.is_empty() {
        return;
    }
    engine.send(Request::Connect {
        id: address.clone(),
        password: String::new(),
        // Профиль:кодек:цветность:кадры:качество — см. `viewer::QualitySpec`.
        // Чёткость важнее плавности: удалённо чаще читают текст, чем смотрят видео.
        quality: "max:auto:444:30:150".to_string(),
    });
    history::remember(&mut state.history, &address);
    engine.send(Request::WatchPeers(state.watched()));
    state.address.clear();
    state.focus = Focus::None;
}

/// Приводит введённый адрес к тому виду, который понимает ядро.
///
/// Адрес показывается по три цифры («497 212 169»), и именно так его
/// копируют и диктуют. Ядру же нужны цифры подряд: с пробелами оно отвечает
/// «ID does not exist».
fn clean_address(input: &str) -> String {
    input.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Сохраняет постоянный пароль и очищает поле.
///
/// Обратно ядро его не отдаёт — хранит только хеш, поэтому показывать в поле
/// нечего и держать введённое дальше незачем.
fn save_password(state: &mut State, engine: &engine::Engine) {
    let password = state.password.trim().to_string();
    if password.is_empty() {
        state.notice("Введите пароль, который хотите задать");
        return;
    }
    engine.send(Request::SetPassword(password));
    state.password.clear();
    state.focus = Focus::None;
    state.notice("Постоянный пароль сохранён");
}

/// Ввод в поле постоянного пароля.
fn password_key(state: &mut State, engine: &engine::Engine, key: &Key<'_>) -> bool {
    match key {
        Key::Character(text) => {
            state.password.push_str(text);
            true
        }
        Key::Backspace => {
            state.password.pop();
            true
        }
        Key::Enter => {
            save_password(state, engine);
            true
        }
        Key::Escape => {
            state.password.clear();
            state.focus = Focus::None;
            true
        }
        _ => false,
    }
}

/// Возвращает `true`, если нужно перерисовать окно.
fn key(
    state: &mut State,
    engine: &engine::Engine,
    event: &tao::event::KeyEvent,
    modifiers: ModifiersState,
) -> bool {
    // Открытое меню закрывается любой клавишей: оно нужно только для мыши.
    if state.menu.take().is_some() {
        return true;
    }
    // Вставку узнаём по расположению клавиши, а не по букве: в русской
    // раскладке Ctrl+V приходит как «м».
    if modifiers.control_key() && event.physical_key == KeyCode::KeyV {
        return paste(state);
    }

    state.wake_caret();
    let key = &event.logical_key;
    match state.focus {
        Focus::None => return false,
        Focus::Password => return password_key(state, engine, key),
        Focus::Address => {}
    }
    match key {
        Key::Character(text) => {
            state.address.push_str(text);
            true
        }
        Key::Space => {
            state.address.push(' ');
            true
        }
        Key::Backspace => {
            state.address.pop();
            true
        }
        Key::Enter => {
            connect(state, engine, None);
            true
        }
        Key::Escape => {
            state.address.clear();
            state.focus = Focus::None;
            true
        }
        _ => false,
    }
}

/// Вставляет текст из буфера обмена в поле, где стоит курсор.
///
/// Если ничего не выбрано, адрес — самое вероятное назначение: обычно ID
/// как раз копируют, чтобы подключиться.
fn paste(state: &mut State) -> bool {
    if state.page != Page::Remote && state.focus != Focus::Password {
        return false;
    }
    if state.focus == Focus::None {
        state.focus = Focus::Address;
    }

    match paste_from_clipboard() {
        Ok(text) => {
            let text = one_line(&text);
            if text.is_empty() {
                state.notice("В буфере обмена нет текста");
                return true;
            }
            match state.focus {
                Focus::Password => state.password.push_str(&text),
                _ => state.address.push_str(&text),
            }
            true
        }
        Err(err) => {
            state.notice(format!("Не удалось вставить: {err}"));
            true
        }
    }
}

/// Оставляет от вставленного одну строку без управляющих символов.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

#[cfg(windows)]
fn paste_from_clipboard() -> Result<String, String> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    const CF_UNICODETEXT: u32 = 13;

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("буфер обмена занят".to_string());
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            return Err("в буфере нет текста".to_string());
        }
        let source = GlobalLock(handle as _) as *const u16;
        if source.is_null() {
            CloseClipboard();
            return Err("буфер обмена недоступен".to_string());
        }

        // Строка заканчивается нулём; длину ищем сами.
        let mut length = 0usize;
        while *source.add(length) != 0 {
            length += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(source, length));

        GlobalUnlock(handle as _);
        CloseClipboard();
        Ok(text)
    }
}

#[cfg(not(windows))]
fn paste_from_clipboard() -> Result<String, String> {
    Err("буфер обмена доступен только в Windows".to_string())
}

#[cfg(windows)]
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("буфер обмена занят".to_string());
        }
        EmptyClipboard();
        let bytes = wide.len() * std::mem::size_of::<u16>();
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle.is_null() {
            CloseClipboard();
            return Err("не хватило памяти".to_string());
        }
        let target = GlobalLock(handle) as *mut u16;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
        GlobalUnlock(handle);
        SetClipboardData(CF_UNICODETEXT, handle as HANDLE);
        CloseClipboard();
    }
    Ok(())
}

#[cfg(not(windows))]
fn copy_to_clipboard(_text: &str) -> Result<(), String> {
    Err("буфер обмена доступен только в Windows".to_string())
}

/// Размеры и области нажатия, посчитанные под текущий размер окна.
struct Layout {
    width: f32,
    height: f32,
    nav: Vec<(Hit, Page)>,
    address: Hit,
    connect: Hit,
    copy_id: Hit,
    cards: Cards,
    settings: Settings,
    controls: [Hit; 3],
    /// Кнопка «Установить» — пока приложение не установлено.
    install: Option<Hit>,
    /// Кнопка «Обновить» — пока на GitHub есть версия новее.
    update: Option<Hit>,
    /// Меню правой кнопки: рамка и пункты, если оно открыто.
    menu: Option<(Hit, Vec<(Hit, Action)>)>,
    /// Высота прокручиваемого содержимого — предел прокрутки.
    content_h: f32,
    /// Верх прокручиваемой области. На главной над ней закреплены строка
    /// подключения и «Это рабочее место», в настройках листается всё.
    viewport_top: f32,
}

impl Layout {
    /// На сколько содержимое можно увести вверх.
    fn max_scroll(&self) -> f32 {
        (self.content_h - (self.height - self.viewport_top)).max(0.0)
    }
}

/// Размеры меню поля ввода.
const MENU_W: f32 = 208.0;
const MENU_ITEM_H: f32 = 34.0;
const MENU_PAD: f32 = 6.0;

/// Прямоугольники страницы настроек.
#[derive(Default)]
struct Settings {
    access: Hit,
    modes: Vec<(Hit, skydesktop_bridge::ApproveMode, &'static str, &'static str)>,
    password: Hit,
    /// Одноразовый пароль — только показать и скопировать.
    temporary: Hit,
    reveal: Hit,
    copy: Hit,
    rotate: Hit,
    /// Поле для нового постоянного пароля.
    password_field: Hit,
    save: Hit,
    /// Карточка прав и её переключатель.
    rights: Hit,
    elevate: Hit,
}

/// Прямоугольники карточек — чтобы отрисовка и попадания совпадали.
#[derive(Default)]
struct Cards {
    connect: Hit,
    workplace: Hit,
    active: Option<Hit>,
    active_rows: Vec<(Hit, i32)>,
    history: Option<Hit>,
    history_rows: Vec<(Hit, Hit, usize)>,
}

impl Layout {
    fn zone_at(&self, position: PhysicalPosition<f64>, scale: f64) -> Zone {
        let (x, y) = (position.x / scale, position.y / scale);

        // Меню лежит поверх всего, поэтому проверяется первым.
        if let Some((frame, items)) = &self.menu {
            for (hit, action) in items {
                if hit.contains(x, y) {
                    return Zone::Menu(*action);
                }
            }
            if frame.contains(x, y) {
                return Zone::None;
            }
        }

        for (hit, index) in [
            (self.controls[0], Zone::Minimize),
            (self.controls[1], Zone::Maximize),
            (self.controls[2], Zone::Close),
        ]
        .map(|(hit, zone)| (hit, zone))
        {
            if hit.contains(x, y) {
                return index;
            }
        }
        for (hit, page) in &self.nav {
            if hit.contains(x, y) {
                return Zone::Nav(*page);
            }
        }
        if self.install.map_or(false, |hit| hit.contains(x, y)) {
            return Zone::Install;
        }
        if self.update.map_or(false, |hit| hit.contains(x, y)) {
            return Zone::Update;
        }
        if self.address.contains(x, y) {
            return Zone::Address;
        }
        if self.connect.contains(x, y) {
            return Zone::Connect;
        }
        if self.copy_id.contains(x, y) {
            return Zone::CopyId;
        }
        // Под закреплённую часть строки уезжают при прокрутке — там их не видно,
        // и нажимать по ним нельзя.
        let listed = y >= self.viewport_top as f64;
        for (hit, conn_id) in &self.cards.active_rows {
            if listed && hit.contains(x, y) {
                return Zone::Disconnect(*conn_id);
            }
        }
        for (connect, forget, index) in &self.cards.history_rows {
            if !listed {
                break;
            }
            if connect.contains(x, y) {
                return Zone::HistoryConnect(*index);
            }
            if forget.contains(x, y) {
                return Zone::HistoryForget(*index);
            }
        }
        for (hit, mode, _, _) in &self.settings.modes {
            if hit.contains(x, y) {
                return Zone::Mode(*mode);
            }
        }
        // Глазок лежит внутри поля, поэтому проверяется раньше него:
        // иначе поле забирает нажатие себе и пароль не показать.
        if self.settings.elevate.contains(x, y) {
            return Zone::Elevate;
        }
        for (hit, zone) in [
            (self.settings.reveal, Zone::PasswordReveal),
            (self.settings.temporary, Zone::None),
            (self.settings.copy, Zone::PasswordCopy),
            (self.settings.rotate, Zone::PasswordRotate),
            (self.settings.password_field, Zone::PasswordField),
            (self.settings.save, Zone::PasswordSave),
        ] {
            if hit.contains(x, y) {
                return zone;
            }
        }
        // Свободное место полосы заголовка тянет окно, остальной фон — нет.
        if y < STRIP as f64 && x > SIDEBAR as f64 {
            return Zone::Drag;
        }
        Zone::None
    }
}

fn layout(window: &Window, state: &State, scale: f64) -> Layout {
    layout_for(window.inner_size(), state, scale)
}

fn layout_for(size: PhysicalSize<u32>, state: &State, scale: f64) -> Layout {
    let width = size.width as f32 / scale as f32;
    let height = size.height as f32 / scale as f32;

    let nav = vec![
        (
            Hit {
                x: 12.0,
                y: 128.0,
                w: SIDEBAR - 24.0,
                h: 52.0,
            },
            Page::Remote,
        ),
        (
            Hit {
                x: 12.0,
                y: 188.0,
                w: SIDEBAR - 24.0,
                h: 52.0,
            },
            Page::Settings,
        ),
    ];

    let controls = [
        Hit {
            x: width - 138.0,
            y: 0.0,
            w: 46.0,
            h: STRIP,
        },
        Hit {
            x: width - 92.0,
            y: 0.0,
            w: 46.0,
            h: STRIP,
        },
        Hit {
            x: width - 46.0,
            y: 0.0,
            w: 46.0,
            h: STRIP,
        },
    ];

    let content_x = SIDEBAR + PAD;
    let content_w = (width - SIDEBAR - PAD * 2.0).max(360.0);
    // Карточки сдвигаются целиком: прокрутка — это просто смещение начала.
    // На главной смещается только то, что ниже «Это рабочее место».
    let top = STRIP + 24.0;
    let pinned = state.page == Page::Remote;
    let mut y = if pinned { top } else { top - state.scroll };
    let mut viewport_top = STRIP;

    let mut cards = Cards::default();
    let mut settings = Settings::default();
    // Прямоугольники чужой страницы остаются пустыми: иначе они перехватывают
    // нажатия — карточка «Это рабочее место» накрывала переключатели настроек.
    let mut address = Hit::default();
    let mut connect = Hit::default();
    let mut copy_id = Hit::default();

    if state.page == Page::Remote {
        // Строка подключения.
        cards.connect = Hit {
            x: content_x,
            y,
            w: content_w,
            h: 92.0,
        };
        let button_w = 190.0_f32.min(content_w * 0.28);
        address = Hit {
            x: content_x + 22.0,
            y: y + 21.0,
            w: content_w - 44.0 - button_w - 16.0,
            h: 50.0,
        };
        connect = Hit {
            x: content_x + content_w - 22.0 - button_w,
            y: y + 21.0,
            w: button_w,
            h: 50.0,
        };
        y += cards.connect.h + CARD_GAP;

        // Собственный адрес.
        cards.workplace = Hit {
            x: content_x,
            y,
            w: content_w,
            h: 88.0,
        };
        copy_id = cards.workplace;
        y += cards.workplace.h + CARD_GAP;

        // Отсюда начинается список: он и листается.
        viewport_top = y;
        y -= state.scroll;

        if !state.clients.is_empty() {
            let rows = state.clients.len() as f32;
            let card = Hit {
                x: content_x,
                y,
                w: content_w,
                h: HEAD_H + rows * ROW_H,
            };
            for (index, client) in state.clients.iter().enumerate() {
                let row_y = y + HEAD_H + index as f32 * ROW_H;
                cards.active_rows.push((
                    Hit {
                        x: content_x + content_w - 22.0 - 190.0,
                        y: row_y + (ROW_H - 42.0) / 2.0,
                        w: 190.0,
                        h: 42.0,
                    },
                    client.conn_id,
                ));
            }
            cards.active = Some(card);
            y += card.h + CARD_GAP;
        }

        let rows = state.history.len().max(1) as f32;
        let card = Hit {
            x: content_x,
            y,
            w: content_w,
            h: HEAD_H + TABLE_HEAD_H + rows * ROW_H,
        };
        for index in 0..state.history.len() {
            let row_y = y + HEAD_H + TABLE_HEAD_H + index as f32 * ROW_H;
            let icon_y = row_y + (ROW_H - 30.0) / 2.0;
            cards.history_rows.push((
                Hit {
                    x: content_x + content_w * 0.897,
                    y: icon_y,
                    w: 30.0,
                    h: 30.0,
                },
                Hit {
                    x: content_x + content_w * 0.940,
                    y: icon_y,
                    w: 30.0,
                    h: 30.0,
                },
                index,
            ));
        }
        cards.history = Some(card);
        y += card.h;
    
    } else {

        let modes = [
            (
                skydesktop_bridge::ApproveMode::Click,
                "Подтверждать вручную",
                "Каждое подключение нужно разрешить во всплывающей плашке.",
            ),
            (
                skydesktop_bridge::ApproveMode::Password,
                "По паролю",
                "Подключаются сразу, зная постоянный пароль.",
            ),
            (
                skydesktop_bridge::ApproveMode::Both,
                "Пароль, иначе подтверждение",
                "Без пароля запрос всё равно придёт на этот компьютер.",
            ),
        ];

        settings.access = Hit {
            x: content_x,
            y,
            w: content_w,
            h: HEAD_H + modes.len() as f32 * 58.0 + 12.0,
        };
        for (index, (mode, title, hint)) in modes.into_iter().enumerate() {
            settings.modes.push((
                Hit {
                    x: content_x + 14.0,
                    y: y + HEAD_H + 6.0 + index as f32 * 58.0,
                    w: content_w - 28.0,
                    h: 54.0,
                },
                mode,
                title,
                hint,
            ));
        }
        y += settings.access.h + CARD_GAP;

        settings.password = Hit {
            x: content_x,
            y,
            w: content_w,
            h: HEAD_H + 214.0,
        };
        let button_w = 124.0;
        let field_w = |right: f32| (right - 12.0 - (content_x + 22.0)).max(140.0);

        // Строка одноразового пароля: показать, скопировать, обновить.
        let row = y + HEAD_H + 58.0;
        settings.rotate = Hit {
            x: content_x + content_w - 22.0 - button_w,
            y: row,
            w: button_w,
            h: 46.0,
        };
        settings.copy = Hit {
            x: settings.rotate.x - 10.0 - button_w,
            y: row,
            w: button_w,
            h: 46.0,
        };
        settings.temporary = Hit {
            x: content_x + 22.0,
            y: row,
            w: field_w(settings.copy.x),
            h: 46.0,
        };
        settings.reveal = Hit {
            x: settings.temporary.x + settings.temporary.w - 40.0,
            y: row + 7.0,
            w: 32.0,
            h: 32.0,
        };

        // Строка постоянного пароля: ввод и сохранение.
        let row = y + HEAD_H + 146.0;
        settings.save = Hit {
            x: content_x + content_w - 22.0 - button_w,
            y: row,
            w: button_w,
            h: 46.0,
        };
        settings.password_field = Hit {
            x: content_x + 22.0,
            y: row,
            w: field_w(settings.save.x),
            h: 46.0,
        };

        y += settings.password.h + CARD_GAP;
        settings.rights = Hit {
            x: content_x,
            y,
            w: content_w,
            h: HEAD_H + 78.0,
        };
        settings.elevate = Hit {
            x: content_x + 14.0,
            y: y + HEAD_H + 6.0,
            w: content_w - 28.0,
            h: 60.0,
        };
        y += settings.rights.h;
    }

    // Сколько места занимает содержимое без учёта прокрутки: по нему
    // ограничивается сама прокрутка, чтобы за последней карточкой не было
    // пустоты.
    let content_h = if pinned {
        y + state.scroll - viewport_top + 24.0
    } else {
        y + state.scroll - top + 24.0
    };

    // Меню строится последним: оно должно уместиться в окне целиком.
    let menu = state.menu.map(|menu| {
        let actions = [Action::Paste, Action::Copy, Action::Cut, Action::Clear];
        let frame = Hit {
            x: menu.x.min(width - MENU_W - 8.0).max(8.0),
            y: menu
                .y
                .min(height - (actions.len() as f32 * MENU_ITEM_H + MENU_PAD * 2.0) - 8.0)
                .max(8.0),
            w: MENU_W,
            h: actions.len() as f32 * MENU_ITEM_H + MENU_PAD * 2.0,
        };
        let items = actions
            .into_iter()
            .enumerate()
            .map(|(index, action)| {
                (
                    Hit {
                        x: frame.x + 4.0,
                        y: frame.y + MENU_PAD + index as f32 * MENU_ITEM_H,
                        w: frame.w - 8.0,
                        h: MENU_ITEM_H,
                    },
                    action,
                )
            })
            .collect();
        (frame, items)
    });

    Layout {
        width,
        height,
        nav,
        address,
        connect,
        copy_id,
        cards,
        settings,
        controls,
        // Над чертой с состоянием внизу боковой панели.
        install: install_hit(state, height),
        // Над «Установить», если она есть, иначе на её месте.
        update: (cfg!(windows) && state.device.update_version.is_some()).then(|| {
            let lift = if install_hit(state, height).is_some() { 70.0 } else { 0.0 };
            Hit {
                x: 12.0,
                y: height - 106.0 - 76.0 - lift,
                w: SIDEBAR - 24.0,
                h: 60.0,
            }
        }),
        menu,
        content_h,
        viewport_top,
    }
}

fn install_hit(state: &State, height: f32) -> Option<Hit> {
    (cfg!(windows) && !state.device.installed).then(|| Hit {
        x: 12.0,
        y: height - 106.0 - 76.0,
        w: SIDEBAR - 24.0,
        h: 60.0,
    })
}

fn primary_scale(target: &EventLoopWindowTarget<CoreEvent>) -> f64 {
    target
        .primary_monitor()
        .or_else(|| target.available_monitors().next())
        .map(|m| m.scale_factor())
        .unwrap_or(1.0)
}

/// Размер и положение окна с учётом экрана, на котором его открывают.
struct Placement {
    size: PhysicalSize<u32>,
    min_size: PhysicalSize<u32>,
    position: PhysicalPosition<i32>,
}

impl Placement {
    fn for_screen(target: &EventLoopWindowTarget<CoreEvent>, scale: f64) -> Self {
        let monitor = target
            .primary_monitor()
            .or_else(|| target.available_monitors().next());
        let screen = monitor
            .as_ref()
            .map(MonitorHandle::size)
            .unwrap_or(PhysicalSize::new(1280, 800));
        let origin = monitor
            .as_ref()
            .map(MonitorHandle::position)
            .unwrap_or(PhysicalPosition::new(0, 0));

        let max_w = screen.width.saturating_sub(40).max(360);
        let max_h = screen.height.saturating_sub(96).max(280);
        let want = |v: f64| (v * scale).round() as u32;

        let width = want(1120.0).min(max_w);
        let height = want(720.0).min(max_h);

        Self {
            size: PhysicalSize::new(width, height),
            min_size: PhysicalSize::new(want(720.0).min(width), want(480.0).min(height)),
            position: PhysicalPosition::new(
                origin.x + ((screen.width.saturating_sub(width)) / 2) as i32,
                origin.y + ((screen.height.saturating_sub(height)) / 3) as i32,
            ),
        }
    }
}


fn draw(
    window: &Window,
    surface: &mut softbuffer::Surface<Rc<Window>, Rc<Window>>,
    fonts: Option<&Fonts>,
    logo: Option<&Pixmap>,
    state: &State,
    scale: f64,
) {
    let size = window.inner_size();
    let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
        return;
    };
    if surface.resize(w, h).is_err() {
        return;
    }
    let Ok(mut buffer) = surface.buffer_mut() else {
        return;
    };
    let Some(mut pixmap) = Pixmap::new(size.width, size.height) else {
        return;
    };
    match fonts {
        Some(fonts) => render(&mut pixmap, fonts, logo, state, size, scale),
        None => pixmap.fill(palette::surface()),
    }
    paint::present(&pixmap, &mut buffer);
    let _ = buffer.present();
}

/// Рисует панель целиком. Окно для этого не нужно — так же рисуются
/// и скриншоты для README (см. тест `screenshots`).
fn render(
    pixmap: &mut Pixmap,
    fonts: &Fonts,
    logo: Option<&Pixmap>,
    state: &State,
    size: PhysicalSize<u32>,
    scale: f64,
) {
    let layout = layout_for(size, state, scale);
    let mut canvas = Canvas {
        pixmap,
        fonts,
        scale: scale as f32,
    };

    canvas.fill(0.0, 0.0, layout.width, layout.height, palette::canvas());
    match state.page {
        Page::Remote => draw_remote(&mut canvas, &layout, state),
        Page::Settings => draw_settings(&mut canvas, &layout, state),
    }

    // Полоса заголовка поверх содержимого: прокрученные карточки должны
    // уходить под неё, а не налезать сверху.
    draw_sidebar(&mut canvas, &layout, logo, state);
    draw_strip(&mut canvas, &layout, state);

    if let Some(text) = state.notice_text() {
        draw_notice(&mut canvas, &layout, text);
    }

    draw_menu(&mut canvas, &layout, state);
}

/// Тонкая обёртка над холстом: переводит логические координаты в пиксели.
struct Canvas<'a> {
    pixmap: &'a mut Pixmap,
    fonts: &'a Fonts,
    scale: f32,
}

impl Canvas<'_> {
    fn px(&self, value: f32) -> f32 {
        value * self.scale
    }

    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: tiny_skia::Color) {
        self.round(x, y, w, h, 0.0, color);
    }

    /// Белая карточка с мягкой тенью снизу.
    fn card(&mut self, hit: Hit) {
        self.round(
            hit.x + 1.0,
            hit.y + 2.0,
            hit.w - 2.0,
            hit.h,
            CARD_RADIUS,
            palette::shadow(),
        );
        self.round(hit.x, hit.y, hit.w, hit.h, CARD_RADIUS, palette::surface());
    }

    /// Значок в квадрате `size` с левым верхним углом в (x, y).
    fn icon(&mut self, draw: icons::Draw, x: f32, y: f32, size: f32, color: tiny_skia::Color) {
        draw(self.pixmap, self.px(x), self.px(y), self.px(size), color);
    }

    /// Кнопки окна рисуются от центра.
    fn glyph(&mut self, draw: icons::Draw, cx: f32, cy: f32, size: f32, color: tiny_skia::Color) {
        let size = self.px(size);
        draw(
            self.pixmap,
            self.px(cx) - size / 2.0,
            self.px(cy) - size / 2.0,
            size,
            color,
        );
    }

    fn round(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: tiny_skia::Color) {
        if let Some(rect) = Rect::from_xywh(self.px(x), self.px(y), self.px(w), self.px(h)) {
            paint::rounded_rect(self.pixmap, rect, self.px(radius), color, false);
        }
    }

    fn outline(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: tiny_skia::Color) {
        if let Some(rect) = Rect::from_xywh(
            self.px(x) + 0.5,
            self.px(y) + 0.5,
            self.px(w) - 1.0,
            self.px(h) - 1.0,
        ) {
            paint::rounded_rect(self.pixmap, rect, self.px(radius), color, true);
        }
    }

    fn text(
        &mut self,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        bold: bool,
        color: tiny_skia::Color,
    ) {
        self.fonts.draw(
            self.pixmap,
            text,
            self.px(x),
            self.px(baseline),
            self.px(size),
            bold,
            color,
        );
    }

    fn text_center(
        &mut self,
        text: &str,
        center: f32,
        baseline: f32,
        size: f32,
        bold: bool,
        color: tiny_skia::Color,
    ) {
        let width = self.width(text, size, bold);
        self.text(text, center - width / 2.0, baseline, size, bold, color);
    }

    fn width(&self, text: &str, size: f32, bold: bool) -> f32 {
        self.fonts.width(text, self.px(size), bold) / self.scale
    }

    fn clip(&self, text: &str, size: f32, bold: bool, max: f32) -> String {
        paint::ellipsize(self.fonts, text, self.px(size), bold, self.px(max))
    }
}

fn draw_sidebar(canvas: &mut Canvas, layout: &Layout, logo: Option<&Pixmap>, state: &State) {
    canvas.fill(0.0, 0.0, SIDEBAR, layout.height, palette::surface());
    canvas.fill(SIDEBAR - 1.0, 0.0, 1.0, layout.height, palette::divider());

    // Знак и название.
    match logo {
        Some(logo) => {
            let (x, y, size) = (canvas.px(24.0), canvas.px(32.0), canvas.px(42.0));
            paint::draw_image(canvas.pixmap, logo, x, y, size);
        }
        None => canvas.icon(icons::signal, 24.0, 32.0, 42.0, palette::brand()),
    }
    canvas.text("SkyDesktop", 76.0, 62.0, 23.0, true, palette::text());

    for (hit, page) in &layout.nav {
        let active = state.page == *page;
        if active {
            canvas.round(hit.x, hit.y, hit.w, hit.h, 12.0, palette::brand_soft());
        } else if state.hover == Zone::Nav(*page) {
            canvas.round(hit.x, hit.y, hit.w, hit.h, 12.0, palette::hover_fill());
        }

        let color = if active {
            palette::brand()
        } else {
            palette::text()
        };
        let (icon, label) = match page {
            Page::Remote => (icons::monitor as icons::Draw, "Удалённый рабочий стол"),
            Page::Settings => (icons::gear as icons::Draw, "Настройки"),
        };
        canvas.icon(icon, hit.x + 16.0, hit.y + hit.h / 2.0 - 11.0, 22.0, color);
        let clipped = canvas.clip(label, 13.5, active, hit.w - 62.0);
        canvas.text(
            &clipped,
            hit.x + 48.0,
            hit.y + hit.h / 2.0 + 5.0,
            13.5,
            active,
            color,
        );
    }

    // «Установить»: без установки окна UAC и экран входа удалённой стороне
    // не видны. Кнопка — выбор, а не требование: без неё всё работает, как раньше.
    if let Some(hit) = layout.install {
        let busy = state.device.installing;
        let fill = if busy {
            palette::text_faint()
        } else if state.hover == Zone::Install {
            palette::brand_hover()
        } else {
            palette::brand()
        };
        canvas.round(hit.x, hit.y, hit.w, hit.h, 12.0, fill);
        canvas.icon(icons::shield, hit.x + 16.0, hit.y + hit.h / 2.0 - 11.0, 22.0, palette::surface());
        canvas.text(
            if busy { "Установка…" } else { "Установить" },
            hit.x + 48.0,
            hit.y + 26.0,
            14.0,
            true,
            palette::surface(),
        );
        let hint = if busy {
            "Ждём разрешения Windows"
        } else {
            "Полные права, окна UAC"
        };
        let hint = canvas.clip(hint, 11.5, false, hit.w - 60.0);
        canvas.text(&hint, hit.x + 48.0, hit.y + 44.0, 11.5, false, palette::surface());
    }

    // «Обновить»: на GitHub вышла версия новее этой сборки.
    if let (Some(hit), Some(version)) = (layout.update, state.device.update_version.as_deref()) {
        let busy = state.device.updating;
        let fill = if busy {
            palette::text_faint()
        } else if state.hover == Zone::Update {
            palette::brand_hover()
        } else {
            palette::brand()
        };
        canvas.round(hit.x, hit.y, hit.w, hit.h, 12.0, fill);
        canvas.icon(icons::download, hit.x + 16.0, hit.y + hit.h / 2.0 - 11.0, 22.0, palette::surface());
        canvas.text(
            if busy { "Обновление…" } else { "Обновить" },
            hit.x + 48.0,
            hit.y + 26.0,
            14.0,
            true,
            palette::surface(),
        );
        let hint = if busy {
            "Скачиваем новую версию".to_string()
        } else {
            format!("Доступна версия {version}")
        };
        let hint = canvas.clip(&hint, 11.5, false, hit.w - 60.0);
        canvas.text(&hint, hit.x + 48.0, hit.y + 44.0, 11.5, false, palette::surface());
    }

    // Состояние внизу: точки остаются зелёными.
    let bottom = layout.height - 106.0;
    canvas.fill(20.0, bottom, SIDEBAR - 40.0, 1.0, palette::divider());

    let (status, dot) = match state.device.status {
        DeviceStatus::Ready => ("Готов к подключению", palette::success()),
        DeviceStatus::Connecting => ("Подключение к сети…", palette::text_faint()),
        DeviceStatus::Unconfirmed => ("Сервер не закрепил адрес", palette::warning()),
        DeviceStatus::Offline => ("Нет связи с сервером", palette::text_faint()),
    };
    canvas.round(22.0, bottom + 26.0, 9.0, 9.0, 4.5, dot);
    canvas.text(status, 40.0, bottom + 35.0, 12.5, false, palette::text_muted());

    canvas.round(22.0, bottom + 50.0, 9.0, 9.0, 4.5, palette::success());
    canvas.text(
        "Ядро: подключено",
        40.0,
        bottom + 59.0,
        12.5,
        false,
        palette::text_muted(),
    );
    canvas.text(
        concat!("v", env!("CARGO_PKG_VERSION")),
        22.0,
        bottom + 84.0,
        11.5,
        false,
        palette::text_faint(),
    );
}

fn draw_strip(canvas: &mut Canvas, layout: &Layout, state: &State) {
    // Полоса заголовка — от голубого слева к фирменному синему справа.
    let width = (layout.width - SIDEBAR).max(1.0);
    let steps = width.ceil() as i32;
    for step in 0..steps {
        let t = (step as f32 / width).min(1.0);
        let mix = |from: u8, to: u8| (from as f32 + (to as f32 - from as f32) * t) as u8;
        let color = tiny_skia::Color::from_rgba8(
            mix(0x55, 0x0d),
            mix(0xb9, 0x55),
            mix(0xf7, 0xb8),
            255,
        );
        canvas.fill(SIDEBAR + step as f32, 0.0, 1.5, STRIP, color);
    }

    let zones = [Zone::Minimize, Zone::Maximize, Zone::Close];
    for (index, hit) in layout.controls.iter().enumerate() {
        if state.hover == zones[index] {
            let color = if index == 2 {
                tiny_skia::Color::from_rgba8(0xc4, 0x2b, 0x1c, 255)
            } else {
                tiny_skia::Color::from_rgba8(0x00, 0x00, 0x00, 40)
            };
            canvas.fill(hit.x, hit.y, hit.w, hit.h, color);
        }
        let cx = hit.x + hit.w / 2.0;
        let cy = hit.y + hit.h / 2.0;
        let white = palette::surface();
        match index {
            0 => canvas.glyph(icons::minimize, cx, cy, 12.0, white),
            1 => canvas.glyph(icons::maximize, cx, cy, 11.0, white),
            _ => canvas.glyph(icons::close, cx, cy, 12.0, white),
        }
    }
}

fn draw_remote(canvas: &mut Canvas, layout: &Layout, state: &State) {
    // Сначала листаемый список, потом закреплённая часть поверх него: строки,
    // уехавшие вверх, прячутся под фоном, а не налезают на поле адреса.
    draw_remote_list(canvas, layout, state);
    canvas.fill(
        SIDEBAR,
        STRIP,
        layout.width - SIDEBAR,
        layout.viewport_top - STRIP,
        palette::canvas(),
    );
    draw_remote_pinned(canvas, layout, state);
}

/// Строка подключения и «Это рабочее место» — не листаются.
fn draw_remote_pinned(canvas: &mut Canvas, layout: &Layout, state: &State) {
    let cards = &layout.cards;

    // --- Строка подключения ---
    canvas.card(cards.connect);

    let field = layout.address;
    let active = state.focus == Focus::Address;
    input_frame(canvas, field, active, state.hover == Zone::Address);

    let dot = match state.device.status {
        DeviceStatus::Ready => palette::success(),
        _ => palette::text_faint(),
    };
    canvas.round(field.x + 18.0, field.y + field.h / 2.0 - 5.5, 11.0, 11.0, 5.5, dot);

    let baseline = field.y + field.h / 2.0 + 5.5;
    let text_x = field.x + 42.0;
    let mut caret_x = text_x;
    if state.address.is_empty() {
        // Подсказку показываем, пока в поле не встали: иначе она спорит с курсором.
        if !active {
            let hint = canvas.clip(
                "Введите адрес удалённого устройства или его IP",
                15.0,
                false,
                field.w - 60.0,
            );
            canvas.text(&hint, text_x, baseline, 15.0, false, palette::text_muted());
        }
    } else {
        let shown = canvas.clip(&state.address, 15.5, false, field.w - 60.0);
        canvas.text(&shown, text_x, baseline, 15.5, false, palette::text());
        caret_x += canvas.width(&shown, 15.5, false) + 2.0;
    }
    if active && state.caret_visible() {
        canvas.fill(caret_x, field.y + 13.0, 1.5, field.h - 26.0, palette::text());
    }

    let button = layout.connect;
    let fill = if state.hover == Zone::Connect {
        palette::brand_hover()
    } else {
        palette::brand()
    };
    canvas.round(button.x, button.y, button.w, button.h, 10.0, fill);
    let label = "Подключиться";
    let label_w = canvas.width(label, 15.0, true);
    let start = button.x + (button.w - (label_w + 26.0)) / 2.0;
    canvas.icon(
        icons::bolt,
        start,
        button.y + button.h / 2.0 - 9.0,
        18.0,
        palette::surface(),
    );
    canvas.text(
        label,
        start + 26.0,
        button.y + button.h / 2.0 + 5.5,
        15.0,
        true,
        palette::surface(),
    );

    // --- Собственный адрес ---
    canvas.card(cards.workplace);
    if state.hover == Zone::CopyId {
        let hit = cards.workplace;
        canvas.round(hit.x + 8.0, hit.y + 8.0, hit.w - 16.0, hit.h - 16.0, 10.0, palette::brand_soft());
    }
    let id = format_id(&state.device.id);
    let label = "Это рабочее место";
    let label_w = canvas.width(label, 18.0, false);
    let id_w = canvas.width(&id, 32.0, true);
    let start = cards.workplace.x + (cards.workplace.w - (label_w + 26.0 + id_w)) / 2.0;
    let mid = cards.workplace.y + cards.workplace.h / 2.0;
    canvas.text(label, start, mid + 6.0, 18.0, false, palette::text_muted());
    canvas.text(&id, start + label_w + 26.0, mid + 11.0, 32.0, true, palette::brand());
}

/// Активные подключения и история — листаются.
fn draw_remote_list(canvas: &mut Canvas, layout: &Layout, state: &State) {
    let cards = &layout.cards;

    // --- Активные подключения ---
    if let Some(card) = cards.active {
        canvas.card(card);
        section_head(canvas, card, icons::wifi, "Активные подключения");

        for (index, client) in state.clients.iter().enumerate() {
            let y = card.y + HEAD_H + index as f32 * ROW_H;
            let mid = y + ROW_H / 2.0;
            let name = if client.name.is_empty() {
                "неизвестный пользователь".to_string()
            } else {
                client.name.clone()
            };

            avatar(canvas, card.x + 24.0, mid - 17.0, 34.0, &name);
            let prefix = "Сейчас подключён ";
            canvas.text(prefix, card.x + 70.0, mid + 5.0, 14.5, false, palette::text_muted());
            let offset = canvas.width(prefix, 14.5, false);
            canvas.text(&name, card.x + 70.0 + offset, mid + 5.0, 14.5, true, palette::text());

            if let Some((hit, _)) = cards.active_rows.get(index) {
                let hovered = state.hover == Zone::Disconnect(client.conn_id);
                let fill = if hovered {
                    palette::brand_soft()
                } else {
                    palette::surface()
                };
                canvas.round(hit.x, hit.y, hit.w, hit.h, 9.0, fill);
                canvas.outline(hit.x, hit.y, hit.w, hit.h, 9.0, palette::brand());

                let label = "Прервать доступ";
                let label_w = canvas.width(label, 14.0, true);
                let start = hit.x + (hit.w - (label_w + 24.0)) / 2.0;
                canvas.icon(icons::stop, start, hit.y + hit.h / 2.0 - 7.0, 14.0, palette::brand());
                canvas.text(
                    label,
                    start + 24.0,
                    hit.y + hit.h / 2.0 + 5.0,
                    14.0,
                    true,
                    palette::brand(),
                );
            }
        }
    }

    // --- История подключений ---
    let Some(card) = cards.history else {
        return;
    };
    canvas.card(card);
    section_head(canvas, card, icons::clock, "История подключений");

    let head_y = card.y + HEAD_H;
    canvas.fill(card.x + 1.0, head_y, card.w - 2.0, TABLE_HEAD_H, palette::field());
    for (title, x) in [
        ("УСТРОЙСТВО", card.x + 24.0),
        ("АДРЕС", card.x + card.w * 0.41),
        ("ПОСЛЕДНЕЕ ПОДКЛЮЧЕНИЕ", card.x + card.w * 0.665),
    ] {
        canvas.text(title, x, head_y + TABLE_HEAD_H / 2.0 + 4.0, 11.0, true, palette::text_faint());
    }

    if state.history.is_empty() {
        canvas.text_center(
            "История пуста — подключитесь к первому устройству",
            card.x + card.w / 2.0,
            head_y + TABLE_HEAD_H + ROW_H / 2.0 + 5.0,
            14.0,
            false,
            palette::text_muted(),
        );
        return;
    }

    for (index, visit) in state.history.iter().enumerate() {
        let y = head_y + TABLE_HEAD_H + index as f32 * ROW_H;
        let mid = y + ROW_H / 2.0;
        if index > 0 {
            canvas.fill(card.x + 24.0, y, card.w - 48.0, 1.0, palette::divider());
        }

        // Вместо значка — присутствие: сразу видно, к кому можно подключиться.
        let (dot, ring) = match state.is_online(&visit.address) {
            Some(true) => (palette::success(), palette::success_soft()),
            Some(false) => (palette::text_faint(), palette::hover_fill()),
            None => (palette::divider(), palette::hover_fill()),
        };
        canvas.round(card.x + 24.0, mid - 11.0, 22.0, 22.0, 11.0, ring);
        canvas.round(card.x + 30.0, mid - 5.0, 10.0, 10.0, 5.0, dot);

        // Адрес показан в соседнем столбце — повторять его здесь незачем.
        let (name, color) = if visit.name.is_empty() {
            ("Имя не получено".to_string(), palette::text_faint())
        } else {
            (visit.name.clone(), palette::text())
        };
        let clipped = canvas.clip(&name, 14.0, false, card.w * 0.41 - 92.0);
        canvas.text(&clipped, card.x + 58.0, mid + 5.0, 14.0, false, color);
        canvas.text(
            &format_id(&visit.address),
            card.x + card.w * 0.41,
            mid + 5.0,
            14.0,
            false,
            palette::text(),
        );
        canvas.text(
            &visit.when,
            card.x + card.w * 0.665,
            mid + 5.0,
            13.5,
            false,
            palette::text_muted(),
        );

        if let Some((connect, forget, _)) = cards.history_rows.get(index) {
            if state.hover == Zone::HistoryConnect(index) {
                canvas.round(connect.x, connect.y, connect.w, connect.h, 8.0, palette::brand_soft());
            }
            canvas.icon(icons::bolt, connect.x + 8.0, connect.y + 7.0, 16.0, palette::brand());

            if state.hover == Zone::HistoryForget(index) {
                canvas.round(forget.x, forget.y, forget.w, forget.h, 8.0, palette::hover_fill());
            }
            canvas.icon(icons::trash, forget.x + 8.0, forget.y + 7.0, 16.0, palette::text_faint());
        }
    }
}

/// Кружок с инициалами — так строки списка легче различать взглядом.
fn avatar(canvas: &mut Canvas, x: f32, y: f32, size: f32, name: &str) {
    canvas.round(x, y, size, size, size / 2.0, palette::brand_soft());
    let font = size * 0.38;
    canvas.text_center(
        &initials(name),
        x + size / 2.0,
        y + size / 2.0 + font * 0.36,
        font,
        true,
        palette::brand(),
    );
}

fn draw_settings(canvas: &mut Canvas, layout: &Layout, state: &State) {
    let settings = &layout.settings;

    // --- Как разрешать подключения ---
    canvas.card(settings.access);
    section_head(canvas, settings.access, icons::gear, "Доступ к этому компьютеру");

    for (hit, mode, title, hint) in &settings.modes {
        let chosen = state.device.approve_mode == *mode;
        if chosen {
            canvas.round(hit.x, hit.y, hit.w, hit.h, 11.0, palette::brand_soft());
        } else if state.hover == Zone::Mode(*mode) {
            canvas.round(hit.x, hit.y, hit.w, hit.h, 11.0, palette::hover_fill());
        }

        let cx = hit.x + 32.0;
        let cy = hit.y + hit.h / 2.0;
        let ring = if chosen {
            palette::brand()
        } else {
            palette::text_faint()
        };
        canvas.round(cx - 9.0, cy - 9.0, 18.0, 18.0, 9.0, ring);
        canvas.round(cx - 7.0, cy - 7.0, 14.0, 14.0, 7.0, palette::surface());
        if chosen {
            canvas.round(cx - 4.0, cy - 4.0, 8.0, 8.0, 4.0, palette::brand());
        }

        let color = if chosen {
            palette::brand()
        } else {
            palette::text()
        };
        canvas.text(title, hit.x + 54.0, cy - 2.0, 14.5, chosen, color);
        canvas.text(hint, hit.x + 54.0, cy + 16.0, 12.5, false, palette::text_muted());
    }

    // --- Пароли ---
    let card = settings.password;
    canvas.card(card);
    section_head(canvas, card, icons::key, "Пароль для подключения");

    let needs = state.device.approve_mode != skydesktop_bridge::ApproveMode::Click;
    let explain = if needs {
        "Пароль спрашивают у того, кто подключается к этому компьютеру."
    } else {
        "Сейчас не используется: подключение разрешается вручную в плашке."
    };
    canvas.text(
        explain,
        card.x + 24.0,
        card.y + HEAD_H + 20.0,
        13.0,
        false,
        palette::text_muted(),
    );

    // Одноразовый: ядро отдаёт его как есть, поэтому его можно показать.
    canvas.text(
        "Одноразовый — меняется при перезапуске",
        card.x + 24.0,
        card.y + HEAD_H + 48.0,
        12.5,
        false,
        palette::text_faint(),
    );

    let field = settings.temporary;
    canvas.round(field.x, field.y, field.w, field.h, 10.0, palette::field());
    canvas.outline(field.x, field.y, field.w, field.h, 10.0, palette::divider());

    let temporary = &state.device.password;
    let baseline = field.y + field.h / 2.0 + 5.5;
    if temporary.is_empty() {
        canvas.text(
            "ядро ещё не выдало пароль",
            field.x + 18.0,
            baseline,
            14.0,
            false,
            palette::text_muted(),
        );
    } else {
        let shown = if state.reveal {
            temporary.clone()
        } else {
            "•".repeat(temporary.chars().count())
        };
        canvas.text(&shown, field.x + 18.0, baseline, 15.5, false, palette::text());
    }

    // Глазок показывает пароль, пока он спрятан звёздочками.
    let eye = settings.reveal;
    if state.hover == Zone::PasswordReveal {
        canvas.round(eye.x, eye.y, eye.w, eye.h, 8.0, palette::hover_fill());
    }
    let (glyph, eye_color) = if state.reveal {
        (icons::hidden as icons::Draw, palette::brand())
    } else {
        (icons::eye as icons::Draw, palette::text_faint())
    };
    canvas.icon(glyph, eye.x + 7.0, eye.y + 8.0, 17.0, eye_color);

    // Постоянный: ядро хранит его хешем и обратно не отдаёт, поэтому поле
    // всегда пустое — здесь его только задают заново.
    let set = state.device.password_kind == skydesktop_bridge::PasswordKind::Permanent;
    canvas.text(
        if set {
            "Постоянный — задан, введите новый, чтобы заменить"
        } else {
            "Постоянный — не задан"
        },
        card.x + 24.0,
        card.y + HEAD_H + 136.0,
        12.5,
        false,
        palette::text_faint(),
    );

    let field = settings.password_field;
    let active = state.focus == Focus::Password;
    input_frame(canvas, field, active, state.hover == Zone::PasswordField);

    let baseline = field.y + field.h / 2.0 + 5.5;
    let mut caret_x = field.x + 18.0;
    if state.password.is_empty() {
        if !active {
            canvas.text(
                "Новый постоянный пароль",
                field.x + 18.0,
                baseline,
                14.0,
                false,
                palette::text_muted(),
            );
        }
    } else {
        let shown = canvas.clip(&state.password, 15.5, false, field.w - 36.0);
        canvas.text(&shown, field.x + 18.0, baseline, 15.5, false, palette::text());
        caret_x += canvas.width(&shown, 15.5, false) + 2.0;
    }
    if active && state.caret_visible() {
        canvas.fill(caret_x, field.y + 13.0, 1.5, field.h - 26.0, palette::text());
    }

    draw_rights(canvas, settings, state);

    for (hit, zone, label, primary) in [
        (settings.copy, Zone::PasswordCopy, "Копировать", false),
        (settings.rotate, Zone::PasswordRotate, "Обновить", false),
        (settings.save, Zone::PasswordSave, "Сохранить", true),
    ] {
        let hovered = state.hover == zone;
        if primary {
            let fill = if hovered {
                palette::brand_hover()
            } else {
                palette::brand()
            };
            canvas.round(hit.x, hit.y, hit.w, hit.h, 10.0, fill);
            canvas.text_center(
                label,
                hit.x + hit.w / 2.0,
                hit.y + hit.h / 2.0 + 5.0,
                14.0,
                true,
                palette::surface(),
            );
        } else {
            let fill = if hovered {
                palette::brand_soft()
            } else {
                palette::surface()
            };
            canvas.round(hit.x, hit.y, hit.w, hit.h, 10.0, fill);
            canvas.outline(hit.x, hit.y, hit.w, hit.h, 10.0, palette::brand());
            canvas.text_center(
                label,
                hit.x + hit.w / 2.0,
                hit.y + hit.h / 2.0 + 5.0,
                14.0,
                true,
                palette::brand(),
            );
        }
    }
}

/// Рамка поля ввода: в фокусе она белая с двойной синей обводкой.
///
/// Разница между «сюда можно печатать» и «просто надпись» должна быть видна
/// без наведения, поэтому активное поле светлее фона карточки.
fn input_frame(canvas: &mut Canvas, field: Hit, active: bool, hovered: bool) {
    let fill = if active {
        palette::surface()
    } else {
        palette::field()
    };
    canvas.round(field.x, field.y, field.w, field.h, 10.0, fill);

    if active {
        canvas.outline(field.x, field.y, field.w, field.h, 10.0, palette::brand());
        canvas.outline(
            field.x + 1.0,
            field.y + 1.0,
            field.w - 2.0,
            field.h - 2.0,
            9.0,
            palette::brand(),
        );
    } else {
        let border = if hovered {
            palette::text_faint()
        } else {
            palette::divider()
        };
        canvas.outline(field.x, field.y, field.w, field.h, 10.0, border);
    }
}

/// Меню правой кнопки над полем ввода.
fn draw_menu(canvas: &mut Canvas, layout: &Layout, state: &State) {
    let Some((frame, items)) = &layout.menu else {
        return;
    };

    // Лёгкая тень, чтобы меню читалось поверх карточки.
    canvas.round(frame.x + 1.0, frame.y + 3.0, frame.w - 2.0, frame.h, 11.0, palette::shadow());
    canvas.round(frame.x, frame.y, frame.w, frame.h, 11.0, palette::surface());
    canvas.outline(frame.x, frame.y, frame.w, frame.h, 11.0, palette::divider());

    let empty = state.field(state.focus).is_empty();
    for (hit, action) in items {
        // Копировать, вырезать и очистить нечего, пока поле пустое.
        let enabled = *action == Action::Paste || !empty;
        if enabled && state.hover == Zone::Menu(*action) {
            canvas.round(hit.x, hit.y, hit.w, hit.h, 8.0, palette::brand_soft());
        }

        let color = if enabled {
            palette::text()
        } else {
            palette::text_faint()
        };
        let baseline = hit.y + hit.h / 2.0 + 4.5;
        canvas.text(action.label(), hit.x + 14.0, baseline, 13.5, false, color);

        let hint = action.hint();
        let width = canvas.width(hint, 12.0, false);
        canvas.text(
            hint,
            hit.x + hit.w - 14.0 - width,
            baseline,
            12.0,
            false,
            palette::text_faint(),
        );
    }
}

/// Карточка «Права в системе».
fn draw_rights(canvas: &mut Canvas, settings: &Settings, state: &State) {
    let card = settings.rights;
    canvas.card(card);
    section_head(canvas, card, icons::shield, "Права в системе");

    let hit = settings.elevate;
    let on = state.device.elevate_on_start;
    if state.hover == Zone::Elevate {
        canvas.round(hit.x, hit.y, hit.w, hit.h, 11.0, palette::hover_fill());
    }

    // Квадратик с галочкой: включено или нет, видно без наведения.
    let box_size = 20.0;
    let bx = hit.x + 18.0;
    let by = hit.y + hit.h / 2.0 - box_size / 2.0;
    if on {
        canvas.round(bx, by, box_size, box_size, 6.0, palette::brand());
        canvas.icon(icons::check, bx + 3.0, by + 3.0, box_size - 6.0, palette::surface());
    } else {
        canvas.round(bx, by, box_size, box_size, 6.0, palette::surface());
        canvas.outline(bx, by, box_size, box_size, 6.0, palette::text_faint());
    }

    let text_x = hit.x + 52.0;
    let mid = hit.y + hit.h / 2.0;
    canvas.text(
        "Запрашивать права администратора при запуске",
        text_x,
        mid - 2.0,
        14.5,
        on,
        if on { palette::brand() } else { palette::text() },
    );

    // Честно говорим, что даёт настройка и что она умеет не всё.
    let hint = if state.device.installed {
        "Установлено: служба видит окна UAC и экран входа"
    } else if state.device.elevated {
        "Сейчас приложение уже работает с полными правами"
    } else if on {
        "Windows спросит разрешение при следующем запуске"
    } else {
        "Без них не проходят действия в окнах, запущенных от администратора"
    };
    canvas.text(hint, text_x, mid + 18.0, 12.5, false, palette::text_muted());
}

/// Короткое сообщение внизу окна.
fn draw_notice(canvas: &mut Canvas, layout: &Layout, text: &str) {
    let width = canvas.width(text, 13.5, false) + 40.0;
    let x = SIDEBAR + (layout.width - SIDEBAR - width) / 2.0;
    let y = layout.height - 62.0;
    canvas.round(x, y, width, 38.0, 11.0, palette::text());
    canvas.text_center(text, x + width / 2.0, y + 24.0, 13.5, false, palette::surface());
}

/// Заголовок раздела: значок, название и отчёркивание.
fn section_head(canvas: &mut Canvas, card: Hit, icon: icons::Draw, title: &str) {
    canvas.icon(icon, card.x + 24.0, card.y + 20.0, 22.0, palette::brand());
    canvas.text(title, card.x + 58.0, card.y + 37.0, 16.5, true, palette::text());
    canvas.fill(card.x + 1.0, card.y + HEAD_H - 1.0, card.w - 2.0, 1.0, palette::divider());
}

/// Девятизначный адрес разбиваем по три цифры — так его проще прочитать вслух.
pub fn format_id(id: &str) -> String {
    let trimmed = id.trim();
    let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 9 && digits.len() == trimmed.len() {
        format!("{} {} {}", &digits[0..3], &digits[3..6], &digits[6..9])
    } else {
        trimmed.to_string()
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_loses_spaces_before_core() {
        // Ровно так адрес выглядит в окне и в буфере обмена.
        assert_eq!(clean_address("497 212 169"), "497212169");
        assert_eq!(clean_address("  497212169\n"), "497212169");
        assert_eq!(clean_address("192.168.0.5"), "192.168.0.5");
    }

    #[test]
    fn nine_digits_are_grouped_for_reading() {
        assert_eq!(format_id("497212169"), "497 212 169");
        // Адреса другой длины и хосты остаются как есть.
        assert_eq!(format_id("192.168.0.5"), "192.168.0.5");
    }
}

/// Скриншоты для README на выдуманных данных — настоящие адрес, пароль и
/// история в репозиторий не попадают:
/// `cargo test -p skydesktop-app -- --ignored screenshots`.
#[cfg(test)]
mod screenshots {
    use super::*;

    fn demo() -> State {
        let mut state = State::new();
        state.device = DeviceStateDto {
            id: "123456789".to_string(),
            password: "k7m2qx".to_string(),
            password_kind: skydesktop_bridge::PasswordKind::Temporary,
            status: DeviceStatus::Ready,
            approve_mode: skydesktop_bridge::ApproveMode::Both,
            latency_ms: Some(18),
            directory: skydesktop_bridge::DirectoryStateDto {
                url: "https://nodes.example.com/nodes".to_string(),
                nodes: vec!["rs1.example.com".to_string(), "rs2.example.com".to_string()],
                relays: vec!["relay1.example.com".to_string()],
                error: String::new(),
            },
            simulated: false,
            elevate_on_start: false,
            elevated: true,
            installed: true,
            installing: false,
            update_version: None,
            updating: false,
        };
        state.history = [
            ("482915377", "BUH-PC", "17.10.2026, 14:32"),
            ("730164258", "Сервер 1С", "16.10.2026, 09:15"),
            ("615208943", "Ноутбук директора", "12.10.2026, 18:47"),
        ]
        .into_iter()
        .map(|(address, name, when)| Visit {
            address: address.to_string(),
            name: name.to_string(),
            when: when.to_string(),
        })
        .collect();
        state.online = HashMap::from([
            ("482915377".to_string(), true),
            ("730164258".to_string(), true),
            ("615208943".to_string(), false),
        ]);
        state
    }

    fn save(state: &State, name: &str) {
        let fonts = Fonts::load().expect("шрифты вшиты в exe");
        let logo = Pixmap::decode_png(LOGO).ok();
        let scale = 1.5;
        let size = PhysicalSize::new((1180.0 * scale) as u32, (760.0 * scale) as u32);
        let mut pixmap = Pixmap::new(size.width, size.height).unwrap();
        render(&mut pixmap, &fonts, logo.as_ref(), state, size, scale);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots");
        std::fs::create_dir_all(&dir).unwrap();
        pixmap.save_png(dir.join(name)).unwrap();
    }

    /// Длинная история, чтобы было что листать.
    fn long_history(state: &mut State) {
        let first = state.history[0].clone();
        state.history = (0..14)
            .map(|i| Visit {
                address: format!("{}", 400000000 + i * 1234567),
                name: format!("{} {}", first.name, i + 1),
                when: first.when.clone(),
            })
            .collect();
    }

    #[test]
    fn workplace_stays_put_while_history_scrolls() {
        let mut state = demo();
        long_history(&mut state);
        let size = PhysicalSize::new(1180, 760);
        let still = layout_for(size, &state, 1.0);
        assert!(still.max_scroll() > 0.0, "длинную историю должно быть можно листать");

        state.scroll = still.max_scroll();
        let scrolled = layout_for(size, &state, 1.0);
        // Закреплённое — на тех же местах.
        assert_eq!(scrolled.address.y, still.address.y);
        assert_eq!(scrolled.cards.workplace.y, still.cards.workplace.y);
        assert_eq!(scrolled.viewport_top, still.viewport_top);
        // История уехала ровно на величину прокрутки.
        let (before, after) = (still.cards.history.unwrap(), scrolled.cards.history.unwrap());
        assert_eq!(after.y, before.y - state.scroll);
        // Последняя строка докручивается до низа окна, но не дальше.
        let last = scrolled.cards.history_rows.last().unwrap().0;
        assert!(last.y + last.h <= size.height as f32);
        // Строка, спрятанная под закреплённой частью, не нажимается.
        let hidden = scrolled.cards.history_rows[0].0;
        assert!(hidden.y < scrolled.viewport_top);
        let point = PhysicalPosition::new((hidden.x + hidden.w / 2.0) as f64, (hidden.y + 2.0) as f64);
        assert_ne!(scrolled.zone_at(point, 1.0), Zone::HistoryConnect(0));
    }

    #[test]
    #[ignore = "пишет файлы в docs/screenshots"]
    fn scrolled() {
        let mut state = demo();
        long_history(&mut state);
        state.scroll = 400.0;
        save(&state, "../../core/target/scrolled.png");
    }

    #[test]
    #[ignore = "пишет файлы в docs/screenshots"]
    fn screenshots() {
        let mut state = demo();
        save(&state, "panel.png");

        state.page = Page::Settings;
        save(&state, "settings.png");

        state.page = Page::Remote;
        state.device.update_version = Some("0.3.0".to_string());
        save(&state, "update.png");
    }
}
