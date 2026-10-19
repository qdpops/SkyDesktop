//! Окно удалённого сеанса: настоящая картинка удалённого экрана
//! и настоящее управление мышью и клавиатурой.
//!
//! Кадры приходят из ядра через [`InvokeUiSession::on_rgba`], события ввода
//! уходят обратно через `Session::send_mouse` / `Session::input_key`.
//! Один сеанс — один процесс (`SkyDesktop.exe --connect <адрес>`), как и в ядре:
//! так сбой сеанса не роняет панель управления.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, RwLock};

use hbb_common::message_proto::*;
use hbb_common::rendezvous_proto::ConnType;
use librustdesk::client::QualityStatus;
use librustdesk::ui_session_interface::{io_loop, InvokeUiSession, Session};
use skydesktop_bridge::SessionMetricsDto;

use tiny_skia::Pixmap;

use crate::paint::Hit;

use tao::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    keyboard::{KeyCode, ModifiersState},
    window::{CursorIcon, Fullscreen, Window, WindowBuilder},
};

/// Тип события мыши в маске протокола (нижние 3 бита).
const MOUSE_TYPE_MOVE: i32 = 0;
const MOUSE_TYPE_DOWN: i32 = 1;
const MOUSE_TYPE_UP: i32 = 2;
const MOUSE_TYPE_WHEEL: i32 = 3;
/// Кнопки кодируются старшими битами: `button << 3`.
const MOUSE_BUTTON_LEFT: i32 = 0x01;
const MOUSE_BUTTON_RIGHT: i32 = 0x02;
const MOUSE_BUTTON_WHEEL: i32 = 0x04;

/// Цвет полей вокруг кадра.
const LETTERBOX: u32 = 0x0011_1213;

/// Высота шторки и полоса у верхнего края, которая её вызывает.
const BAR_H: f32 = 44.0;
const BAR_W: f32 = 520.0;
const REVEAL: f64 = 4.0;

/// Что показывать, пока изображения ещё нет.
const WAITING_DEFAULT: &str = "Подключаемся к удалённому компьютеру…";

/// Последний кадр удалённого экрана.
#[derive(Default)]
struct Frame {
    /// Пиксели в порядке B, G, R, A — так их отдаёт декодер ядра.
    bgra: Vec<u8>,
    width: usize,
    height: usize,
}

/// Сообщения из потока сеанса в цикл окна.
#[derive(Debug, Clone)]
enum ViewerEvent {
    /// Пришёл новый кадр.
    Frame,
    /// Ядро сообщило размер удалённого экрана.
    Display { width: u32, height: u32 },
    /// Пришёл первый кадр — сеанс полностью установлен.
    Connected,
    /// Сеанс завершён.
    Closed(String),
    /// Сообщение ядра (ошибка входа, неверный пароль и т. п.).
    Notice { title: String, text: String },
    /// Показатели канала — уходят в заголовок окна.
    Quality(String),
    /// Что писать на заставке до первого кадра.
    Waiting(String),
    /// Изменился список мониторов или активный монитор — шторку перерисовать.
    Displays,
}

/// Приёмник событий сеанса. Ядро его клонирует, поэтому всё состояние — в `Arc`.
#[derive(Clone, Default)]
pub struct ViewerHandler {
    /// Адрес удалённой стороны — уходит в панель вместе с показателями.
    address: Arc<Mutex<String>>,
    frame: Arc<Mutex<Frame>>,
    proxy: Arc<Mutex<Option<EventLoopProxy<ViewerEvent>>>>,
    /// Размер удалённого экрана; нужен для пересчёта координат мыши.
    remote_size: Arc<Mutex<(u32, u32)>>,
    /// Начало текущего экрана в общем рабочем столе удалённой машины.
    remote_origin: Arc<Mutex<(i32, i32)>>,
    /// Что написано на заставке, пока не пришёл первый кадр.
    waiting: Arc<Mutex<String>>,
    /// Мониторы удалённой машины: сколько их и какой сейчас показываем.
    displays: Arc<Mutex<Displays>>,
}

/// Мониторы удалённой машины.
#[derive(Default, Clone, Copy)]
struct Displays {
    count: usize,
    current: usize,
}

impl ViewerHandler {
    fn notify(&self, event: ViewerEvent) {
        if let Some(proxy) = self.proxy.lock().unwrap().as_ref() {
            let _ = proxy.send_event(event);
        }
    }
}

/// Переводит сообщение ядра на человеческий язык.
///
/// Ядро отдаёт английские строки, а причина отказа — это первое, что видит
/// пользователь, когда окно сеанса закрывается.
fn explain(text: &str) -> String {
    const TABLE: &[(&str, &str)] = &[
        ("Remote desktop is offline", "устройство не в сети"),
        ("ID does not exist", "такого адреса нет"),
        ("Failed to connect to relay server", "сервер-посредник недоступен"),
        ("Failed to connect via relay server", "не удалось связаться через посредника"),
        ("Failed to make direct connection to remote desktop", "прямое подключение не вышло"),
        ("Connection refused", "подключение отклонено"),
        ("Wrong Password", "неверный пароль"),
        ("No Password Access", "доступ по паролю запрещён"),
        ("Timeout", "истекло время ожидания"),
        ("Offline", "устройство не в сети"),
    ];

    for (english, russian) in TABLE {
        if text.contains(english) {
            return russian.to_string();
        }
    }
    text.to_string()
}

impl InvokeUiSession for ViewerHandler {
    fn on_rgba(&self, display: usize, rgba: &mut scrap::ImageRgb) {
        // Сразу после переключения ещё могут дойти кадры прежнего монитора:
        // их не показываем, иначе картинка мигнёт старым экраном.
        if display != self.displays.lock().unwrap().current {
            return;
        }
        {
            let mut frame = self.frame.lock().unwrap();
            frame.width = rgba.w;
            frame.height = rgba.h;
            frame.bgra.clear();
            frame.bgra.extend_from_slice(&rgba.raw);
        }
        self.notify(ViewerEvent::Frame);
    }

    fn set_display(&self, x: i32, y: i32, w: i32, h: i32, _cursor_embedded: bool, _scale: f64) {
        let (w, h) = (w.max(1) as u32, h.max(1) as u32);
        tracing::info!("экран удалённой машины: {w}x{h} в точке {x},{y}");
        *self.remote_size.lock().unwrap() = (w, h);
        // Удалённая сторона применяет координаты мыши как есть, в общем
        // пространстве рабочего стола. Для второго монитора его начало не
        // равно нулю, и без этой поправки щелчки уходили бы на первый.
        *self.remote_origin.lock().unwrap() = (x, y);
        self.notify(ViewerEvent::Display {
            width: w,
            height: h,
        });
    }

    fn msgbox(&self, msgtype: &str, title: &str, text: &str, _link: &str, _retry: bool) {
        tracing::info!("ядро: [{msgtype}] {title} — {text}");

        // "input-password" — не отказ: ядро сообщает, что вход идёт без пароля
        // и его сейчас подтвердят на той стороне. Сеанс при этом продолжается.
        if msgtype == "input-password" {
            self.notify(ViewerEvent::Waiting(
                "Ожидаем, пока на том компьютере примут подключение".to_string(),
            ));
            return;
        }

        // Ядро прямо говорит, что решение принимает удалённая сторона.
        if msgtype.starts_with("wait-remote-accept") {
            self.notify(ViewerEvent::Waiting(
                "Ожидаем, пока на том компьютере примут подключение".to_string(),
            ));
            return;
        }

        // А вот это уже отказ: пароль нужен и введённый не подошёл.
        if msgtype == "re-input-password" {
            crate::metrics::report_password_required(self.address.lock().unwrap().clone(), true);
            self.notify(ViewerEvent::Closed("нужен пароль".to_string()));
            return;
        }

        if msgtype.contains("error") {
            // Окно сеанса вот-вот закроется, поэтому причину показывает
            // панель: иначе подключение выглядит как мигнувшее окно.
            crate::metrics::report_failed(
                self.address.lock().unwrap().clone(),
                explain(text),
            );
            self.notify(ViewerEvent::Notice {
                title: title.to_string(),
                text: text.to_string(),
            });
        }
    }

        /// Ядро зовёт это, когда пришёл первый кадр: пора убрать
    /// сообщение «ожидание изображения», а не закрывать окно.
    fn close_success(&self) {
        self.notify(ViewerEvent::Connected);
    }

    fn update_quality_status(&self, qs: QualityStatus) {
        let fps = qs.fps.values().next().copied();
        let codec = qs.codec_format.map(|c| c.to_string()).unwrap_or_default();

        let mut parts = Vec::new();
        if let Some(speed) = qs.speed.clone() {
            parts.push(speed);
        }
        if let Some(fps) = fps {
            parts.push(format!("{fps} FPS"));
        }
        if let Some(delay) = qs.delay {
            parts.push(format!("{delay} мс"));
        }
        if !codec.is_empty() {
            parts.push(codec.clone());
        }
        if !parts.is_empty() {
            self.notify(ViewerEvent::Quality(parts.join(" · ")));
        }

        crate::metrics::report(SessionMetricsDto {
            address: self.address.lock().unwrap().clone(),
            speed: qs.speed.unwrap_or_default(),
            delay_ms: qs.delay,
            fps,
            target_bitrate_kbps: qs.target_bitrate,
            codec,
            chroma: qs.chroma.unwrap_or_default(),
        });
    }

    // --- остальное ядру от этого окна не требуется ---

    fn set_cursor_data(&self, _cd: CursorData) {}
    fn set_cursor_id(&self, _id: String) {}
    fn set_cursor_position(&self, _cp: CursorPosition) {}
    fn switch_display(&self, display: &SwitchDisplay) {
        // Удалённая сторона подтвердила переключение (или переключила сама).
        let mut displays = self.displays.lock().unwrap();
        displays.current = display.display.max(0) as usize;
        drop(displays);
        self.notify(ViewerEvent::Displays);
    }
    fn set_peer_info(&self, peer_info: &PeerInfo) {
        {
            let mut displays = self.displays.lock().unwrap();
            displays.count = peer_info.displays.len();
            displays.current = (peer_info.current_display.max(0) as usize)
                .min(displays.count.saturating_sub(1));
        }
        self.notify(ViewerEvent::Displays);

        // Имя удалённого компьютера известно только отсюда — в истории
        // подключений иначе остался бы один адрес.
        let name = if peer_info.hostname.is_empty() {
            peer_info.username.clone()
        } else {
            peer_info.hostname.clone()
        };
        if !name.is_empty() {
            crate::metrics::report_peer_name(self.address.lock().unwrap().clone(), name);
        }
    }
    /// Мониторы подключили или отключили уже во время сеанса.
    fn set_displays(&self, list: &Vec<DisplayInfo>) {
        {
            let mut displays = self.displays.lock().unwrap();
            displays.count = list.len();
            displays.current = displays.current.min(list.len().saturating_sub(1));
        }
        self.notify(ViewerEvent::Displays);
    }
    fn set_platform_additions(&self, _data: &str) {}
    fn on_connected(&self, _conn_type: ConnType) {}
    fn update_privacy_mode(&self) {}
    fn set_permission(&self, _name: &str, _value: bool) {}
    fn set_connection_type(&self, _is_secured: bool, _direct: bool, _stream_type: &str) {}
    fn set_fingerprint(&self, _fingerprint: String) {}
    fn job_error(&self, _id: i32, _err: String, _file_num: i32) {}
    fn job_done(&self, _id: i32, _file_num: i32) {}
    fn clear_all_jobs(&self) {}
    fn new_message(&self, _msg: String) {}
    fn update_transfer_list(&self) {}
    fn load_last_job(&self, _cnt: i32, _job_json: &str, _auto_start: bool) {}
    fn update_folder_files(
        &self,
        _id: i32,
        _entries: &Vec<FileEntry>,
        _path: String,
        _is_local: bool,
        _only_count: bool,
    ) {
    }
    fn confirm_delete_files(&self, _id: i32, _i: i32, _name: String) {}
    fn override_file_confirm(
        &self,
        _id: i32,
        _file_num: i32,
        _to: String,
        _is_upload: bool,
        _is_identical: bool,
    ) {
    }
    fn update_block_input_state(&self, _on: bool) {}
    fn job_progress(&self, _id: i32, _file_num: i32, _speed: f64, _finished_size: f64) {}
    fn adapt_size(&self) {}
    fn cancel_msgbox(&self, _tag: &str) {}
    fn switch_back(&self, _id: &str) {}
    fn portable_service_running(&self, _running: bool) {}
    fn on_voice_call_started(&self) {}
    fn on_voice_call_closed(&self, _reason: &str) {}
    fn on_voice_call_waiting(&self) {}
    fn on_voice_call_incoming(&self) {}
    /// Кадры рисуются прямо в [`Self::on_rgba`] — хранить их для ядра не нужно.
    fn get_rgba(&self, _display: usize) -> *const u8 {
        std::ptr::null()
    }
    fn next_rgba(&self, _display: usize) {}
    fn set_multiple_windows_session(&self, _sessions: Vec<WindowsSession>) {}
    fn set_current_display(&self, _disp_idx: i32) {}
    fn update_record_status(&self, _start: bool) {}
    fn printer_request(&self, _id: i32, _path: String) {}
    fn handle_screenshot_resp(&self, _sid: String, _msg: String) {}
    fn handle_terminal_response(&self, _response: TerminalResponse) {}
}

/// Открывает окно сеанса и не возвращает управление до его закрытия.
///
/// `quality` — профиль из панели: он решает, разрешаем ли мы 4:4:4.
/// Окончательный выбор всё равно за адаптивным контроллером удалённой стороны.
pub fn run(id: String, password: String, quality: String, args: Vec<String>) -> ! {
    let quality = QualitySpec::parse(&quality);
    quality.store_peer_options(&id);

    let event_loop = EventLoopBuilder::<ViewerEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let handler = ViewerHandler::default();
    *handler.proxy.lock().unwrap() = Some(proxy);
    *handler.address.lock().unwrap() = id.clone();
    *handler.waiting.lock().unwrap() = WAITING_DEFAULT.to_string();

    let session: Session<ViewerHandler> = Session {
        password,
        args,
        server_keyboard_enabled: Arc::new(RwLock::new(true)),
        server_file_transfer_enabled: Arc::new(RwLock::new(true)),
        server_clipboard_enabled: Arc::new(RwLock::new(true)),
        reconnect_count: Arc::new(AtomicUsize::new(0)),
        ui_handler: handler.clone(),
        ..Default::default()
    };
    session.lc.write().unwrap().initialize(
        id.clone(),
        ConnType::DEFAULT_CONN,
        None,
        false,
        None,
        None,
        None,
    );

    // Сетевой цикл сеанса — в своём потоке; окно остаётся отзывчивым.
    // Его завершение и есть конец сеанса: тогда закрываем окно.
    {
        let session = session.clone();
        let handler = handler.clone();
        std::thread::spawn(move || {
            let round = session.connection_round_state.lock().unwrap().new_round();
            io_loop(session, round);
            tracing::info!("сетевой цикл сеанса завершился");
            handler.notify(ViewerEvent::Closed("соединение завершено".to_string()));
        });
    }

    // Окно живёт и в замыкании цикла, и в поверхности отрисовки — отсюда Rc.
    let window = Rc::new(
        WindowBuilder::new()
            .with_title(format!("SkyDesktop — {}", crate::panel::format_id(&id)))
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_window_icon(app_icon())
            .build(&event_loop)
            .expect("не удалось создать окно сеанса"),
    );

    let context = softbuffer::Context::new(window.clone()).expect("нет графического контекста");
    let mut surface = softbuffer::Surface::new(&context, window.clone())
        .expect("не удалось создать поверхность");

    // Нужны только заставке до первого кадра.
    let fonts = crate::paint::Fonts::load();

    // Состояние ввода: положение курсора и зажатые модификаторы.
    let mut cursor = PhysicalPosition::new(0.0f64, 0.0f64);
    let mut modifiers = ModifiersState::empty();
    // Шторка управления: видна, пока курсор у верхнего края или на ней самой.
    let mut bar_shown = false;
    let mut bar_hover: Option<BarButton> = None;
    // Где стоит шторка: доля ширины окна под её серединой. Её можно утащить
    // вбок, если она закрывает такую же шторку удалённого RDP.
    let mut bar_center = 0.5f32;
    // Пока шторку тащат — на сколько курсор правее её левого края.
    let mut bar_drag: Option<f32> = None;
    let mut bar_grab_cursor = false;

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(ViewerEvent::Frame) => window.request_redraw(),
            Event::UserEvent(ViewerEvent::Connected) => {
                quality.apply_to_session(&session);
                window.set_title(&format!("SkyDesktop — {}", crate::panel::format_id(&id)));
                window.request_redraw();
            }
            Event::UserEvent(ViewerEvent::Display { width, height }) => {
                // В полном экране размер окна задаёт монитор, а не удалённый экран:
                // при переключении на монитор другого разрешения окно не трогаем.
                if window.fullscreen().is_none() {
                    window.set_inner_size(fit_to_screen(&window, width, height));
                }
                window.request_redraw();
            }
            Event::UserEvent(ViewerEvent::Displays) => window.request_redraw(),
            Event::UserEvent(ViewerEvent::Quality(text)) => {
                window.set_title(&format!("SkyDesktop — {} · {text}", crate::panel::format_id(&id)));
            }
            Event::UserEvent(ViewerEvent::Waiting(text)) => {
                *handler.waiting.lock().unwrap() = text;
                window.request_redraw();
            }
            Event::UserEvent(ViewerEvent::Notice { title, text }) => {
                window.set_title(&format!("SkyDesktop — {title}: {text}"));
            }
            Event::UserEvent(ViewerEvent::Closed(reason)) => {
                tracing::info!("сеанс закрыт: {reason}");
                *control_flow = ControlFlow::Exit;
            }

            Event::RedrawRequested(_) => {
                let displays = *handler.displays.lock().unwrap();
                let overlay = bar_shown.then(|| {
                    Overlay::new(window.inner_size(), scale_of(&window), bar_center, displays.count)
                });
                present(
                    &handler,
                    &window,
                    &mut surface,
                    fonts.as_ref(),
                    overlay.as_ref().map(|o| BarState {
                        overlay: o,
                        hovered: bar_hover,
                        current_display: displays.current,
                        fullscreen: window.fullscreen().is_some(),
                    }),
                    &id,
                );
            }

            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    tracing::info!("окно сеанса закрыто пользователем");
                    session.close();
                    *control_flow = ControlFlow::Exit;
                }
                WindowEvent::Resized(_) => {
                    // Разворот окна сразу уводит в полный экран: развёрнутое
                    // окно всё равно упирается в рамку и панель задач, а кадр
                    // из-за этого пересчитывается вместо показа один к одному.
                    // Сначала снимаем разворот — иначе выход из полного экрана
                    // вернул бы развёрнутое окно и тут же ушёл обратно.
                    if window.is_maximized() && window.fullscreen().is_none() {
                        window.set_maximized(false);
                        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                        bar_shown = false;
                        bar_hover = None;
                        bar_drag = None;
                    }
                    window.request_redraw();
                }
                WindowEvent::ModifiersChanged(state) => modifiers = state,
                WindowEvent::CursorMoved { position, .. } => {
                    // Верхняя кромка вызывает шторку — и в полном экране, и в окне:
                    // на ней переключаются мониторы удалённой машины.
                    {
                        let size = window.inner_size();
                        let scale = scale_of(&window);
                        let monitors = handler.displays.lock().unwrap().count;
                        // Шторку тащат: она едет за курсором, куда бы тот ни ушёл.
                        if let Some(grab) = bar_drag {
                            bar_center = Overlay::center_for(
                                size,
                                scale,
                                monitors,
                                position.x as f32 - grab,
                            );
                            cursor = position;
                            window.request_redraw();
                            return;
                        }
                        let overlay = Overlay::new(size, scale, bar_center, monitors);
                        let show = overlay.should_show(position);
                        let hover = show.then(|| overlay.button_at(position.x, position.y)).flatten();
                        if show != bar_shown || hover != bar_hover {
                            bar_shown = show;
                            bar_hover = hover;
                            window.request_redraw();
                        }
                        // Над свободным местом шторки курсор подсказывает,
                        // что её можно передвинуть.
                        let grab = show && overlay.is_handle(position.x, position.y);
                        if grab != bar_grab_cursor {
                            bar_grab_cursor = grab;
                            window.set_cursor_icon(if grab {
                                CursorIcon::Move
                            } else {
                                CursorIcon::Default
                            });
                        }
                        // Пока курсор на шторке, удалённой стороне он не нужен.
                        if show && overlay.bar.contains(position.x, position.y) {
                            cursor = position;
                            return;
                        }
                    }
                    if bar_grab_cursor && !bar_shown {
                        bar_grab_cursor = false;
                        window.set_cursor_icon(CursorIcon::Default);
                    }

                    cursor = position;
                    // Точка может попасть в поля вокруг кадра — тогда слать нечего.
                    if let Some((x, y)) = to_remote(&handler, &window, position) {
                        send_mouse(&session, MOUSE_TYPE_MOVE, x, y, modifiers);
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    // Кнопки шторки нажимаются здесь и на удалённую сторону
                    // не уходят.
                    if bar_drag.is_some() && button == MouseButton::Left {
                        if state == ElementState::Released {
                            bar_drag = None;
                        }
                        return;
                    }
                    if bar_shown {
                        let monitors = handler.displays.lock().unwrap().count;
                        let overlay = Overlay::new(
                            window.inner_size(),
                            scale_of(&window),
                            bar_center,
                            monitors,
                        );
                        if overlay.bar.contains(cursor.x, cursor.y) {
                            if state == ElementState::Pressed {
                                if button == MouseButton::Left
                                    && overlay.is_handle(cursor.x, cursor.y)
                                {
                                    bar_drag = Some(cursor.x as f32 - overlay.bar.x);
                                } else {
                                    match overlay.button_at(cursor.x, cursor.y) {
                                        Some(BarButton::Disconnect) => {
                                            tracing::info!("сеанс закрыт кнопкой на шторке");
                                            *control_flow = ControlFlow::Exit;
                                        }
                                        Some(BarButton::Monitor(index)) => {
                                            switch_monitor(&handler, &session, index);
                                        }
                                        Some(BarButton::Windowed) => {
                                            window.set_fullscreen(
                                                if window.fullscreen().is_some() {
                                                    None
                                                } else {
                                                    Some(Fullscreen::Borderless(None))
                                                },
                                            );
                                        }
                                        // Сворачиваем прямо из полного экрана: окно
                                        // вернётся в него же.
                                        Some(BarButton::Minimize) => {
                                            window.set_minimized(true);
                                        }
                                        None => {}
                                    }
                                    bar_shown = false;
                                    bar_hover = None;
                                    window.request_redraw();
                                }
                            }
                            return;
                        }
                    }

                    if let (Some(button), Some((x, y))) =
                        (mouse_button(button), to_remote(&handler, &window, cursor))
                    {
                        let kind = match state {
                            ElementState::Pressed => MOUSE_TYPE_DOWN,
                            _ => MOUSE_TYPE_UP,
                        };
                        send_mouse(&session, button << 3 | kind, x, y, modifiers);
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    // Протокол ждёт «шаги колеса», а не пиксели.
                    let (dx, dy) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (x as i32, y as i32),
                        MouseScrollDelta::PixelDelta(p) => {
                            ((p.x / 40.0) as i32, (p.y / 40.0) as i32)
                        }
                        _ => (0, 0),
                    };
                    if dx != 0 || dy != 0 {
                        send_mouse(
                            &session,
                            MOUSE_BUTTON_WHEEL << 3 | MOUSE_TYPE_WHEEL,
                            dx,
                            dy,
                            modifiers,
                        );
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    // F11 — наша клавиша: разворачивает окно на весь монитор
                    // без рамки. При совпадающих разрешениях это даёт кадр
                    // один к одному, без единого пересчёта.
                    if event.physical_key == KeyCode::F11 {
                        if event.state == ElementState::Pressed {
                            let full = window.fullscreen().is_some();
                            window.set_fullscreen(if full {
                                None
                            } else {
                                Some(Fullscreen::Borderless(None))
                            });
                            bar_shown = false;
                            bar_hover = None;
                            window.request_redraw();
                        }
                        return;
                    }
                    send_key(&session, &event, modifiers);
                }
                _ => {}
            },

            _ => {}
        }
    })
}

/// Параметры качества, выбранные в панели.
///
/// Приходят одной строкой `профиль:кодек:цветность:fps:качество` — остальные
/// аргументы командной строки разбирает само ядро.
#[derive(Debug, Clone)]
pub struct QualitySpec {
    profile: String,
    codec: String,
    i444: bool,
    fps: i32,
    /// Качество в шкале ядра: 50 — обычное, больше — чётче.
    quality: i32,
}

impl QualitySpec {
    fn parse(spec: &str) -> Self {
        let mut parts = spec.split(':');
        let profile = parts.next().unwrap_or("auto").to_string();
        let codec = parts.next().unwrap_or("auto").to_string();
        let chroma = parts.next().unwrap_or("444");
        // 30 кадров вместо 60: при том же битрейте каждый кадр вдвое
        // подробнее, а для работы с текстом и окнами частота не важна.
        let fps = parts.next().and_then(|v| v.parse().ok()).unwrap_or(30);
        // 150 — та же полка, что у «наилучшего качества» ядра (BR_BEST = 1.5).
        let quality = parts.next().and_then(|v| v.parse().ok()).unwrap_or(150);

        // 4:4:4 просим там, где важна чёткость текста и графики. Итоговое
        // решение всё равно за удалённой стороной: она может его не уметь.
        let i444 = chroma == "444" && matches!(profile.as_str(), "auto" | "max");

        Self {
            profile,
            codec,
            i444,
            fps: fps.clamp(5, 144),
            quality: quality.clamp(10, 2000),
        }
    }

    /// Настройки пира читаются ядром при согласовании кодека
    /// (`scrap::codec::preference`), поэтому пишутся до старта сеанса.
    fn store_peer_options(&self, id: &str) {
        use hbb_common::config::PeerConfig;

        let codec = match self.codec.as_str() {
            "av1" => "av1",
            "h265" => "h265",
            "h264" => "h264",
            // Пустое значение — ядро выбирает само.
            _ => "",
        };

        let mut config = PeerConfig::load(id);
        config
            .options
            .insert("i444".to_string(), if self.i444 { "Y" } else { "" }.to_string());
        config
            .options
            .insert("codec-preference".to_string(), codec.to_string());
        config
            .options
            .insert("custom-fps".to_string(), self.fps.to_string());
        config.store(id);
    }

    /// Применяется, когда сеанс установлен: эти параметры уходят сообщением
    /// удалённой стороне, до подключения отправлять их некуда.
    fn apply_to_session(&self, session: &Session<ViewerHandler>) {
        if self.profile == "off" {
            // Ничего не навязываем: качеством распоряжается ядро.
            return;
        }
        // Ядро всё равно снижает битрейт, когда канал не тянет, поэтому
        // высокая планка безопасна: на быстрой сети текст читаемый, на
        // медленной сеанс сам опускается до посильного.
        session.save_image_quality("custom".to_string());
        session.save_custom_image_quality(self.quality);
        session.set_custom_fps(self.fps);
        tracing::info!(
            "качество сеанса: {}% битрейта, {} кадр/с, 4:4:4 {}",
            self.quality,
            self.fps,
            if self.i444 { "да" } else { "нет" }
        );
    }
}

/// Просит удалённую сторону показывать другой монитор.
///
/// Активный монитор отмечаем сразу, не дожидаясь подтверждения: иначе кадры
/// нового монитора, пришедшие раньше ответа, отбрасывались бы как чужие.
fn switch_monitor(handler: &ViewerHandler, session: &Session<ViewerHandler>, index: usize) {
    {
        let mut displays = handler.displays.lock().unwrap();
        if index >= displays.count || index == displays.current {
            return;
        }
        displays.current = index;
    }
    tracing::info!("переключаемся на монитор {}", index + 1);
    session.switch_display(index as i32);
}

/// Шторка в момент отрисовки.
struct BarState<'a> {
    overlay: &'a Overlay,
    hovered: Option<BarButton>,
    current_display: usize,
    fullscreen: bool,
}

/// Рисует последний кадр, вписывая его в окно с сохранением пропорций.
fn present(
    handler: &ViewerHandler,
    window: &Window,
    surface: &mut softbuffer::Surface<Rc<Window>, Rc<Window>>,
    fonts: Option<&crate::paint::Fonts>,
    overlay: Option<BarState>,
    address: &str,
) {
    let size = window.inner_size();
    let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
    else {
        return;
    };
    if surface.resize(width, height).is_err() {
        return;
    }
    let Ok(mut buffer) = surface.buffer_mut() else {
        return;
    };

    let frame = handler.frame.lock().unwrap();
    if frame.width == 0 || frame.height == 0 || frame.bgra.is_empty() {
        let text = handler.waiting.lock().unwrap().clone();
        drop(frame);
        draw_waiting(&mut buffer, size, fonts, &text);
        if let Some(bar) = &overlay {
            draw_overlay(&mut buffer, size, fonts, bar, address);
        }
        let _ = buffer.present();
        return;
    }

    let placement = Placement::new(size, frame.width as u32, frame.height as u32);
    let stride = frame.bgra.len() / frame.height;

    buffer.fill(LETTERBOX);
    let width = size.width as usize;
    let height = size.height as usize;

    // Способ выборки выбирается по масштабу: у каждого своя цена и своя
    // область, где он выглядит правильно.
    if placement.draw_w == placement.src_w && placement.draw_h == placement.src_h {
        // Один к одному — просто переносим строки, без единого пересчёта.
        for dy in 0..placement.draw_h {
            let y = placement.offset_y + dy;
            if y >= height {
                break;
            }
            let line = y * width;
            let row = dy * stride;
            for dx in 0..placement.draw_w {
                let x = placement.offset_x + dx;
                if x >= width {
                    break;
                }
                let src = row + dx * 4;
                buffer[line + x] = u32::from_le_bytes([
                    frame.bgra[src],
                    frame.bgra[src + 1],
                    frame.bgra[src + 2],
                    0,
                ]);
            }
        }
    } else if placement.draw_w >= placement.src_w && placement.draw_h >= placement.src_h {
        // Увеличение: билинейная интерполяция. Пикселей не прибавится, но
        // ступенек на буквах не будет — в отличие от выбора соседа.
        let cols = linear_taps(placement.draw_w, placement.src_w);
        let rows = linear_taps(placement.draw_h, placement.src_h);

        for (dy, &(y0, y1, wy)) in rows.iter().enumerate() {
            let y = placement.offset_y + dy;
            if y >= height {
                break;
            }
            let line = y * width;
            let (row0, row1) = (y0 * stride, y1 * stride);

            for (dx, &(x0, x1, wx)) in cols.iter().enumerate() {
                let x = placement.offset_x + dx;
                if x >= width {
                    break;
                }
                let [b, g, r] = bilinear(&frame.bgra, (row0, row1), (x0 * 4, x1 * 4), wx, wy);
                buffer[line + x] = u32::from_le_bytes([b, g, r, 0]);
            }
        }
    } else {
        // Уменьшение: усреднение по площади. Выбор одного пикселя выбрасывал
        // бы целые столбцы, и у букв рвались бы штрихи.
        let cols = spans(placement.draw_w, placement.src_w);
        let rows = spans(placement.draw_h, placement.src_h);

        for (dy, &(sy0, sy1)) in rows.iter().enumerate() {
            let y = placement.offset_y + dy;
            if y >= height {
                break;
            }
            let line = y * width;

            for (dx, &(sx0, sx1)) in cols.iter().enumerate() {
                let x = placement.offset_x + dx;
                if x >= width {
                    break;
                }

                // Больше четырёх выборок по оси картинку уже не улучшают, а на
                // экране 4K заметно нагружают процессор.
                let step_y = ((sy1 - sy0) / 4).max(1);
                let step_x = ((sx1 - sx0) / 4).max(1);

                let (mut b, mut g, mut r, mut count) = (0u32, 0u32, 0u32, 0u32);
                for sy in (sy0..sy1).step_by(step_y) {
                    let row = sy * stride;
                    for sx in (sx0..sx1).step_by(step_x) {
                        // Байты идут B, G, R, A.
                        let src = row + sx * 4;
                        b += frame.bgra[src] as u32;
                        g += frame.bgra[src + 1] as u32;
                        r += frame.bgra[src + 2] as u32;
                        count += 1;
                    }
                }
                if count == 0 {
                    continue;
                }
                buffer[line + x] =
                    u32::from_le_bytes([(b / count) as u8, (g / count) as u8, (r / count) as u8, 0]);
            }
        }
    }
    drop(frame);
    if let Some(bar) = &overlay {
        draw_overlay(&mut buffer, size, fonts, bar, address);
    }
    let _ = buffer.present();
}

/// Цвет точки по четырём соседям кадра.
///
/// `rows` — смещения двух строк, `cols` — двух столбцов (уже в байтах),
/// `wx`/`wy` — доли правого и нижнего соседа в 1/256.
#[inline]
fn bilinear(bgra: &[u8], rows: (usize, usize), cols: (usize, usize), wx: u32, wy: u32) -> [u8; 3] {
    // Смешивание по горизонтали даёт значение, умноженное на 256; после
    // второго смешивания множитель становится 65536, поэтому и сдвиг на 16.
    let mix = |left: u8, right: u8| left as u32 * (256 - wx) + right as u32 * wx;

    let mut channels = [0u8; 3];
    for (i, channel) in channels.iter_mut().enumerate() {
        let top = mix(bgra[rows.0 + cols.0 + i], bgra[rows.0 + cols.1 + i]);
        let bottom = mix(bgra[rows.1 + cols.0 + i], bgra[rows.1 + cols.1 + i]);
        *channel = ((top * (256 - wy) + bottom * wy) >> 16) as u8;
    }
    channels
}

/// Для каждой точки назначения — два соседних пикселя и вес правого.
///
/// Вес в 1/256, чтобы обойтись целочисленной арифметикой: кадров тридцать в
/// секунду, и каждый лишний расчёт с плавающей точкой здесь заметен.
fn linear_taps(dst_len: usize, src_len: usize) -> Vec<(usize, usize, u32)> {
    (0..dst_len)
        .map(|i| {
            // Центр точки назначения в координатах кадра.
            let pos = ((i as f64 + 0.5) * src_len as f64 / dst_len as f64 - 0.5).max(0.0);
            let base = (pos.floor() as usize).min(src_len - 1);
            let next = (base + 1).min(src_len - 1);
            (base, next, ((pos - base as f64) * 256.0).round() as u32)
        })
        .collect()
}

/// Для каждой точки назначения — полуинтервал исходных пикселей.
///
/// При увеличении интервал вырождается в один пиксель, при уменьшении
/// покрывает все, что попали под точку.
fn spans(dst_len: usize, src_len: usize) -> Vec<(usize, usize)> {
    (0..dst_len)
        .map(|i| {
            let from = i * src_len / dst_len;
            let to = ((i + 1) * src_len / dst_len).max(from + 1).min(src_len);
            (from, to)
        })
        .collect()
}

/// Рисует шторку поверх уже нарисованного кадра.
fn draw_overlay(
    buffer: &mut softbuffer::Buffer<Rc<Window>, Rc<Window>>,
    size: PhysicalSize<u32>,
    fonts: Option<&crate::paint::Fonts>,
    state: &BarState,
    address: &str,
) {
    let Some(pixmap) = fonts.and_then(|fonts| bar_pixmap(fonts, state, address)) else {
        return;
    };
    let left = state.overlay.bar.x.round() as i32;
    blend_pixmap(buffer, size.width as usize, left, 0, &pixmap);
}

/// Шторка отдельной картинкой — её же сохраняет тест `screenshots`.
fn bar_pixmap(fonts: &crate::paint::Fonts, state: &BarState, address: &str) -> Option<Pixmap> {
    use crate::paint::palette;

    let overlay = state.overlay;
    let hovered = state.hovered;
    let bar = overlay.bar;
    let (w, h) = (bar.w.round() as u32, bar.h.round() as u32);
    let mut pixmap = Pixmap::new(w.max(1), h.max(1))?;

    // Тёмная подложка со скруглением снизу — над кадром она читается всегда.
    if let Some(rect) = tiny_skia::Rect::from_xywh(0.0, -12.0, bar.w, bar.h + 12.0) {
        crate::paint::rounded_rect(
            &mut pixmap,
            rect,
            12.0,
            tiny_skia::Color::from_rgba8(0x1b, 0x1f, 0x24, 240),
            false,
        );
    }

    let scale = bar.h / BAR_H;

    // Ручка слева: две колонки точек — намёк, что шторку можно передвинуть.
    let dot = 2.0 * scale;
    for column in 0..2 {
        for row in 0..3 {
            if let Some(rect) = tiny_skia::Rect::from_xywh(
                14.0 * scale + column as f32 * 5.0 * scale,
                bar.h / 2.0 - 6.0 * scale + row as f32 * 5.0 * scale,
                dot,
                dot,
            ) {
                crate::paint::rounded_rect(
                    &mut pixmap,
                    rect,
                    dot / 2.0,
                    tiny_skia::Color::from_rgba8(0x8b, 0x94, 0x9e, 255),
                    false,
                );
            }
        }
    }

    let text_size = 14.0 * scale;
    fonts.draw(
        &mut pixmap,
        // Адрес разбит по три цифры — как везде в приложении.
        &format!("SkyDesktop · {}", crate::panel::format_id(address)),
        32.0 * scale,
        bar.h / 2.0 + text_size * 0.36,
        text_size,
        false,
        tiny_skia::Color::from_rgba8(0xe6, 0xed, 0xf3, 255),
    );

    // Кнопки: мониторы, свернуть, полный экран и отключиться.
    for (hit, button) in overlay.buttons() {
        let local_x = hit.x - bar.x;

        // Монитор — квадрат с номером, как в AnyDesk; активный залит цветом.
        if let BarButton::Monitor(index) = button {
            let active = index == state.current_display;
            let inset = 4.0 * scale;
            if let Some(rect) = tiny_skia::Rect::from_xywh(
                local_x + inset,
                hit.y + inset,
                hit.w - inset * 2.0,
                hit.h - inset * 2.0,
            ) {
                let fill = if active {
                    palette::brand()
                } else if hovered == Some(button) {
                    tiny_skia::Color::from_rgba8(0xff, 0xff, 0xff, 40)
                } else {
                    tiny_skia::Color::from_rgba8(0xff, 0xff, 0xff, 16)
                };
                crate::paint::rounded_rect(&mut pixmap, rect, 7.0 * scale, fill, false);
            }
            let label = (index + 1).to_string();
            let label_size = 14.0 * scale;
            let label_w = fonts.width(&label, label_size, true);
            fonts.draw(
                &mut pixmap,
                &label,
                local_x + (hit.w - label_w) / 2.0,
                hit.y + hit.h / 2.0 + label_size * 0.36,
                label_size,
                true,
                palette::surface(),
            );
            continue;
        }

        let icon = match button {
            BarButton::Minimize => crate::icons::minimize as crate::icons::Draw,
            // В окне эта же кнопка разворачивает на весь экран.
            BarButton::Windowed if state.fullscreen => crate::icons::windowed,
            BarButton::Windowed => crate::icons::maximize,
            BarButton::Disconnect | BarButton::Monitor(_) => crate::icons::disconnect,
        };
        if hovered == Some(button) {
            if let Some(rect) = tiny_skia::Rect::from_xywh(local_x, hit.y, hit.w, hit.h) {
                crate::paint::rounded_rect(
                    &mut pixmap,
                    rect,
                    9.0 * scale,
                    tiny_skia::Color::from_rgba8(0xff, 0xff, 0xff, 40),
                    false,
                );
            }
        }
        let glyph = hit.w * 0.5;
        icon(
            &mut pixmap,
            local_x + (hit.w - glyph) / 2.0,
            hit.y + (hit.h - glyph) / 2.0,
            glyph,
            if button == BarButton::Disconnect {
                tiny_skia::Color::from_rgba8(0xff, 0x8a, 0x80, 255)
            } else {
                palette::surface()
            },
        );
    }

    Some(pixmap)
}

/// Накладывает пиксельную карту на буфер окна с учётом прозрачности.
fn blend_pixmap(
    buffer: &mut softbuffer::Buffer<Rc<Window>, Rc<Window>>,
    buffer_width: usize,
    left: i32,
    top: i32,
    pixmap: &Pixmap,
) {
    let height = buffer.len() / buffer_width.max(1);

    for (row, pixels) in pixmap.pixels().chunks(pixmap.width() as usize).enumerate() {
        let y = top + row as i32;
        if y < 0 || y as usize >= height {
            continue;
        }
        let line = y as usize * buffer_width;

        for (column, pixel) in pixels.iter().enumerate() {
            let x = left + column as i32;
            if x < 0 || x as usize >= buffer_width {
                continue;
            }
            let alpha = pixel.alpha() as u32;
            if alpha == 0 {
                continue;
            }

            let index = line + x as usize;
            let under = buffer[index].to_le_bytes();
            // Цвета премультиплицированы, поэтому фон берём долей (255 - alpha).
            let mix = |src: u8, dst: u8| {
                ((src as u32 + dst as u32 * (255 - alpha) / 255).min(255)) as u8
            };
            buffer[index] = u32::from_le_bytes([
                mix(pixel.blue(), under[0]),
                mix(pixel.green(), under[1]),
                mix(pixel.red(), under[2]),
                0,
            ]);
        }
    }
}

/// Заставка до первого кадра.
///
/// Пустой чёрный экран не отличить от зависшего сеанса, поэтому пишем, чего
/// именно ждём: чаще всего — решения на той стороне.
fn draw_waiting(
    buffer: &mut softbuffer::Buffer<Rc<Window>, Rc<Window>>,
    size: PhysicalSize<u32>,
    fonts: Option<&crate::paint::Fonts>,
    text: &str,
) {
    buffer.fill(LETTERBOX);

    let (Some(fonts), Some(mut pixmap)) = (fonts, Pixmap::new(size.width, size.height)) else {
        return;
    };

    // Тот же фон, что и у полей вокруг кадра: заставка не мигает при переходе.
    pixmap.fill(tiny_skia::Color::from_rgba8(0x11, 0x12, 0x13, 255));

    let scale = (size.width as f32 / 1280.0).clamp(0.75, 2.0);
    let center = size.width as f32 / 2.0;
    let middle = size.height as f32 / 2.0;

    let title = "SkyDesktop";
    let title_size = 26.0 * scale;
    let title_width = fonts.width(title, title_size, true);
    fonts.draw(
        &mut pixmap,
        title,
        center - title_width / 2.0,
        middle - 30.0 * scale,
        title_size,
        true,
        tiny_skia::Color::from_rgba8(0xff, 0xff, 0xff, 255),
    );

    let body_size = 16.0 * scale;
    let body_width = fonts.width(text, body_size, false);
    fonts.draw(
        &mut pixmap,
        text,
        center - body_width / 2.0,
        middle + 16.0 * scale,
        body_size,
        false,
        tiny_skia::Color::from_rgba8(0xc9, 0xd1, 0xd9, 255),
    );

    let hint = "Окно закроется само, если подключение отклонят";
    let hint_size = 13.0 * scale;
    let hint_width = fonts.width(hint, hint_size, false);
    fonts.draw(
        &mut pixmap,
        hint,
        center - hint_width / 2.0,
        middle + 48.0 * scale,
        hint_size,
        false,
        tiny_skia::Color::from_rgba8(0x6e, 0x76, 0x81, 255),
    );

    crate::paint::present(&pixmap, buffer);
}

/// Множитель размеров интерфейса окна сеанса.
fn scale_of(window: &Window) -> f32 {
    window.scale_factor() as f32
}

/// Шторка управления поверх кадра в полноэкранном режиме.
///
/// Выход из полного экрана иначе доступен только клавишей, о которой надо
/// знать заранее. Панель ведёт себя как в удалённом рабочем столе Windows:
/// прячется, пока курсор не подведут к верхнему краю. За свободное место
/// её можно передвинуть вдоль края — чтобы не закрывала шторку RDP внутри.
struct Overlay {
    bar: Hit,
    /// Кнопки мониторов удалённой машины; пусто, если монитор один.
    monitors: Vec<Hit>,
    minimize: Hit,
    windowed: Hit,
    disconnect: Hit,
}

/// Кнопка на шторке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarButton {
    /// Показать монитор с этим номером (с нуля).
    Monitor(usize),
    Minimize,
    Windowed,
    Disconnect,
}

impl Overlay {
    /// `center` — доля ширины окна под серединой шторки (0.5 — по центру),
    /// `monitors` — сколько мониторов у удалённой машины.
    fn new(size: PhysicalSize<u32>, scale: f32, center: f32, monitors: usize) -> Self {
        let height = BAR_H * scale;
        let width = Self::width(size, scale, monitors);
        let x = Self::left(size, width, center);

        let button = 36.0 * scale;
        let pad = 8.0 * scale;
        // Мониторы — слева от оконных кнопок, отделены от них лишним отступом.
        let shown = Self::monitor_buttons(monitors);
        let first = x + width - pad * 4.0 - button * 3.0 - shown as f32 * (button + pad);
        let monitors = (0..shown)
            .map(|i| Hit {
                x: first + i as f32 * (button + pad),
                y: (height - button) / 2.0,
                w: button,
                h: button,
            })
            .collect();
        let bar = Hit {
            x,
            y: 0.0,
            w: width,
            h: height,
        };
        Self {
            disconnect: Hit {
                x: x + width - pad - button,
                y: (height - button) / 2.0,
                w: button,
                h: button,
            },
            windowed: Hit {
                x: x + width - pad * 2.0 - button * 2.0,
                y: (height - button) / 2.0,
                w: button,
                h: button,
            },
            minimize: Hit {
                x: x + width - pad * 3.0 - button * 3.0,
                y: (height - button) / 2.0,
                w: button,
                h: button,
            },
            monitors,
            bar,
        }
    }

    /// Сколько кнопок мониторов показывать: при одном мониторе — ни одной.
    fn monitor_buttons(monitors: usize) -> usize {
        if monitors > 1 {
            monitors
        } else {
            0
        }
    }

    /// Кнопки слева направо: мониторы, затем как у обычного окна.
    fn buttons(&self) -> Vec<(Hit, BarButton)> {
        let mut buttons: Vec<_> = self
            .monitors
            .iter()
            .enumerate()
            .map(|(i, hit)| (*hit, BarButton::Monitor(i)))
            .collect();
        buttons.extend([
            (self.minimize, BarButton::Minimize),
            (self.windowed, BarButton::Windowed),
            (self.disconnect, BarButton::Disconnect),
        ]);
        buttons
    }

    fn button_at(&self, x: f64, y: f64) -> Option<BarButton> {
        self.buttons()
            .into_iter()
            .find(|(hit, _)| hit.contains(x, y))
            .map(|(_, button)| button)
    }

    /// Сверх двух мониторов шторка расширяется, чтобы кнопки не наезжали на адрес.
    fn width(size: PhysicalSize<u32>, scale: f32, monitors: usize) -> f32 {
        let extra = Self::monitor_buttons(monitors).saturating_sub(2) as f32 * 44.0;
        ((BAR_W + extra) * scale).min(size.width as f32 - 40.0 * scale).max(1.0)
    }

    /// Левый край шторки; за край окна она не уезжает.
    fn left(size: PhysicalSize<u32>, width: f32, center: f32) -> f32 {
        let room = (size.width as f32 - width).max(0.0);
        (size.width as f32 * center - width / 2.0).clamp(0.0, room)
    }

    /// Середина шторки в долях окна, если её левый край поставить в `left`.
    ///
    /// Храним долю, а не пиксели: после смены разрешения шторка остаётся
    /// примерно там же, где её оставили.
    fn center_for(size: PhysicalSize<u32>, scale: f32, monitors: usize, left: f32) -> f32 {
        let width = Self::width(size, scale, monitors);
        let left = left.clamp(0.0, (size.width as f32 - width).max(0.0));
        ((left + width / 2.0) / size.width.max(1) as f32).clamp(0.0, 1.0)
    }

    /// Точка на шторке, но не на кнопке — за неё шторку и тащат.
    fn is_handle(&self, x: f64, y: f64) -> bool {
        self.bar.contains(x, y) && self.button_at(x, y).is_none()
    }

    /// Курсор у самого верха или уже на шторке — значит её показываем.
    fn should_show(&self, cursor: PhysicalPosition<f64>) -> bool {
        cursor.y <= REVEAL || self.bar.contains(cursor.x, cursor.y)
    }
}

/// Как кадр удалённого экрана расположен внутри окна.
struct Placement {
    offset_x: usize,
    offset_y: usize,
    draw_w: usize,
    draw_h: usize,
    src_w: usize,
    src_h: usize,
}

impl Placement {
    fn new(window: PhysicalSize<u32>, src_w: u32, src_h: u32) -> Self {
        // Кадр занимает окно целиком, сохраняя пропорции: поля по краям
        // раздражают сильнее, чем мягкость увеличенной картинки.
        let scale = (window.width as f64 / src_w as f64).min(window.height as f64 / src_h as f64);
        let draw_w = ((src_w as f64 * scale).round() as usize).max(1);
        let draw_h = ((src_h as f64 * scale).round() as usize).max(1);
        Self {
            offset_x: (window.width as usize).saturating_sub(draw_w) / 2,
            offset_y: (window.height as usize).saturating_sub(draw_h) / 2,
            draw_w,
            draw_h,
            src_w: src_w as usize,
            src_h: src_h as usize,
        }
    }

    /// Пиксель кадра под точкой окна, либо `None` для полей вокруг кадра.
    fn source_pixel(&self, x: usize, y: usize) -> Option<(usize, usize)> {
        let dx = x.checked_sub(self.offset_x)?;
        let dy = y.checked_sub(self.offset_y)?;
        if dx >= self.draw_w || dy >= self.draw_h {
            return None;
        }
        Some((
            (dx * self.src_w / self.draw_w).min(self.src_w - 1),
            (dy * self.src_h / self.draw_h).min(self.src_h - 1),
        ))
    }
}

/// Переводит точку окна в координаты удалённого экрана.
fn to_remote(
    handler: &ViewerHandler,
    window: &Window,
    position: PhysicalPosition<f64>,
) -> Option<(i32, i32)> {
    let (rw, rh) = *handler.remote_size.lock().unwrap();
    if rw == 0 || rh == 0 {
        return None;
    }
    let placement = Placement::new(window.inner_size(), rw, rh);
    let (sx, sy) =
        placement.source_pixel(position.x.max(0.0) as usize, position.y.max(0.0) as usize)?;

    let (ox, oy) = *handler.remote_origin.lock().unwrap();
    Some((sx as i32 + ox, sy as i32 + oy))
}

/// Не даём окну сеанса открыться больше экрана.
/// Размер окна под удалённый экран.
///
/// Один к одному, если экран помещается: тогда кадр не пересчитывается вовсе
/// и текст остаётся таким же чётким, каким его закодировала удалённая
/// сторона. Раньше окно всегда занимало 90 % монитора, и кадр пересчитывался
/// даже когда разрешения совпадали.
fn fit_to_screen(window: &Window, width: u32, height: u32) -> PhysicalSize<u32> {
    let limit = window
        .current_monitor()
        .map(|m| m.size())
        .unwrap_or(PhysicalSize::new(1920, 1080));

    // Рамка и панель задач съедают часть монитора.
    let max_w = limit.width.saturating_sub(16).max(320);
    let max_h = limit.height.saturating_sub(96).max(240);

    if width <= max_w && height <= max_h {
        return PhysicalSize::new(width, height);
    }

    let scale = (max_w as f64 / width as f64).min(max_h as f64 / height as f64);
    PhysicalSize::new((width as f64 * scale) as u32, (height as f64 * scale) as u32)
}

fn mouse_button(button: MouseButton) -> Option<i32> {
    match button {
        MouseButton::Left => Some(MOUSE_BUTTON_LEFT),
        MouseButton::Right => Some(MOUSE_BUTTON_RIGHT),
        MouseButton::Middle => Some(MOUSE_BUTTON_WHEEL),
        _ => None,
    }
}

fn send_mouse(
    session: &Session<ViewerHandler>,
    mask: i32,
    x: i32,
    y: i32,
    modifiers: ModifiersState,
) {
    session.send_mouse(
        mask,
        x,
        y,
        modifiers.alt_key(),
        modifiers.control_key(),
        modifiers.shift_key(),
        modifiers.super_key(),
    );
}

/// Передаёт нажатие физической клавишей — по скан-коду, а не по символу.
///
/// Символ зависит от раскладки: при русской Ctrl+C приходит как Ctrl+«с», и
/// на той стороне сочетание не срабатывает. Скан-код от раскладки не зависит,
/// а символ из него получает уже удалённая система — по своей раскладке.
/// Это режим «map» ядра, тот же, что у RustDesk по умолчанию. Модификаторы
/// идут отдельными нажатиями, поэтому любые сочетания проходят как есть.
#[cfg(windows)]
fn send_key(session: &Session<ViewerHandler>, event: &tao::event::KeyEvent, _modifiers: ModifiersState) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, MapVirtualKeyW, MAPVK_VSC_TO_VK_EX, VK_CAPITAL, VK_NUMLOCK,
    };

    let Some(scancode) = event.physical_key.to_scancode() else {
        tracing::debug!("клавиша без скан-кода: {:?}", event.physical_key);
        return;
    };
    // Код клавиши ядру нужен, чтобы отличить правые Ctrl и Alt от левых:
    // скан-коды у них совпадают с точностью до признака расширенной клавиши.
    let vk = unsafe { MapVirtualKeyW(scancode, MAPVK_VSC_TO_VK_EX) };

    // Биты ядра: 1 — Caps Lock, 2 — Num Lock.
    let toggled = |vk: u16| unsafe { GetKeyState(vk as i32) } & 1 != 0;
    let lock_modes = (toggled(VK_CAPITAL) as i32) << 1 | (toggled(VK_NUMLOCK) as i32) << 2;

    session.handle_flutter_raw_key_event(
        "map",
        "",
        vk as i32,
        scancode as i32,
        lock_modes,
        event.state == ElementState::Pressed,
    );
}

#[cfg(not(windows))]
fn send_key(session: &Session<ViewerHandler>, event: &tao::event::KeyEvent, modifiers: ModifiersState) {
    let Some(name) = key_name(&event.logical_key) else {
        return;
    };
    session.input_key(
        &name,
        event.state == ElementState::Pressed,
        false,
        modifiers.alt_key(),
        modifiers.control_key(),
        modifiers.shift_key(),
        modifiers.super_key(),
    );
}

/// Имя клавиши в терминах ядра: одиночный символ либо ключ из его таблицы
/// (`KEY_MAP` в `client.rs`). Имени, которого нет в таблице, ядро молча
/// не передаёт — так раньше терялись пробел, стрелки и Enter.
#[cfg(not(windows))]
fn key_name(key: &tao::keyboard::Key<'_>) -> Option<String> {
    use tao::keyboard::Key as K;
    let name = match key {
        K::Character(text) => return text.chars().next().map(|c| c.to_string()),
        K::Enter => "VK_RETURN",
        K::Tab => "VK_TAB",
        K::Space => "VK_SPACE",
        K::ArrowDown => "VK_DOWN",
        K::ArrowLeft => "VK_LEFT",
        K::ArrowRight => "VK_RIGHT",
        K::ArrowUp => "VK_UP",
        K::Escape => "VK_ESCAPE",
        K::Backspace => "VK_BACK",
        K::Delete => "VK_DELETE",
        K::Home => "VK_HOME",
        K::End => "VK_END",
        K::PageUp => "VK_PRIOR",
        K::PageDown => "VK_NEXT",
        K::Insert => "VK_INSERT",
        K::Shift => "VK_SHIFT",
        K::Control => "VK_CONTROL",
        K::Alt => "VK_MENU",
        K::Super => "Meta",
        K::CapsLock => "VK_CAPITAL",
        K::F1 => "VK_F1",
        K::F2 => "VK_F2",
        K::F3 => "VK_F3",
        K::F4 => "VK_F4",
        K::F5 => "VK_F5",
        K::F6 => "VK_F6",
        K::F7 => "VK_F7",
        K::F8 => "VK_F8",
        K::F9 => "VK_F9",
        K::F10 => "VK_F10",
        K::F11 => "VK_F11",
        K::F12 => "VK_F12",
        _ => return None,
    };
    Some(name.to_string())
}

/// Иконка окна берётся из ресурсов exe (см. `build.rs`).
#[cfg(windows)]
pub fn app_icon() -> Option<tao::window::Icon> {
    use tao::platform::windows::IconExtWindows;
    tao::window::Icon::from_resource(1, None).ok()
}

#[cfg(not(windows))]
pub fn app_icon() -> Option<tao::window::Icon> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_cover_source_without_gaps() {
        // Уменьшение 1920 -> 1280: каждый исходный столбец должен попасть
        // ровно в один диапазон, иначе часть картинки просто теряется.
        let spans = spans(1280, 1920);
        assert_eq!(spans.len(), 1280);
        assert_eq!(spans[0].0, 0);
        assert_eq!(spans[1279].1, 1920);
        for pair in spans.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "диапазоны должны идти встык");
        }
        for &(from, to) in &spans {
            assert!(from < to && to <= 1920);
        }
    }

    #[test]
    fn one_to_one_taps_are_identity() {
        // При равных размерах интерполяция обязана вернуть исходный пиксель:
        // иначе кадр размывался бы там, где пересчёт вообще не нужен.
        for (index, &(base, _, weight)) in linear_taps(800, 800).iter().enumerate() {
            assert_eq!(base, index);
            assert_eq!(weight, 0);
        }
    }

    #[test]
    fn taps_stay_inside_frame() {
        // Увеличение 1366 -> 1920: обращение за границу кадра — паника в окне сеанса.
        let taps = linear_taps(1920, 1366);
        assert_eq!(taps.len(), 1920);
        for &(base, next, weight) in &taps {
            assert!(base < 1366 && next < 1366);
            assert!(next >= base);
            assert!(weight <= 256);
        }
        assert_eq!(taps[0].0, 0);
    }

    #[test]
    fn bilinear_keeps_a_flat_colour() {
        // Однотонный кадр обязан остаться однотонным при любых весах.
        // Переполнение здесь давало цветные полосы вместо картинки.
        let bgra = vec![0x40u8; 4 * 4];
        for wx in [0, 128, 255] {
            for wy in [0, 128, 255] {
                assert_eq!(bilinear(&bgra, (0, 8), (0, 4), wx, wy), [0x40, 0x40, 0x40]);
            }
        }
    }

    #[test]
    fn bilinear_takes_the_midpoint() {
        // Два пикселя, 0 и 200: ровно посередине ожидаем половину.
        let mut bgra = vec![0u8; 8];
        bgra[4] = 200;
        bgra[5] = 200;
        bgra[6] = 200;
        assert_eq!(bilinear(&bgra, (0, 0), (0, 4), 128, 0), [100, 100, 100]);
        // Крайние веса возвращают исходные пиксели без искажения.
        assert_eq!(bilinear(&bgra, (0, 0), (0, 4), 0, 0), [0, 0, 0]);
        assert_eq!(bilinear(&bgra, (0, 0), (0, 4), 256, 0), [200, 200, 200]);
    }

    #[test]
    fn overlay_buttons_stay_inside_the_bar() {
        // Кнопки считаются от правого края шторки: ошибка здесь увела бы их
        // за пределы подложки, и нажать было бы не по чему.
        let overlay = Overlay::new(PhysicalSize::new(1920, 1080), 1.0, 0.5, 1);

        assert!(overlay.bar.x > 0.0, "шторка по центру, а не от края");
        for (hit, button) in overlay.buttons() {
            assert!(hit.x >= overlay.bar.x);
            assert!(hit.x + hit.w <= overlay.bar.x + overlay.bar.w);
            assert!(hit.y >= overlay.bar.y);
            assert!(hit.y + hit.h <= overlay.bar.y + overlay.bar.h);
            // Середина кнопки узнаётся именно как эта кнопка.
            let center = ((hit.x + hit.w / 2.0) as f64, (hit.y + hit.h / 2.0) as f64);
            assert_eq!(overlay.button_at(center.0, center.1), Some(button));
        }
        // Кнопки не должны налезать друг на друга.
        assert!(overlay.minimize.x + overlay.minimize.w <= overlay.windowed.x);
        assert!(overlay.windowed.x + overlay.windowed.w <= overlay.disconnect.x);
    }

    #[test]
    fn overlay_reveals_only_at_the_top() {
        let overlay = Overlay::new(PhysicalSize::new(1920, 1080), 1.0, 0.5, 1);
        let center = (overlay.bar.x + overlay.bar.w / 2.0) as f64;

        // Верхняя кромка вызывает шторку, середина экрана — нет.
        assert!(overlay.should_show(PhysicalPosition::new(center, 0.0)));
        assert!(overlay.should_show(PhysicalPosition::new(center, 20.0)));
        assert!(!overlay.should_show(PhysicalPosition::new(center, 400.0)));
        // Ниже шторки, но у левого края — тоже нет: там работает удалённый экран.
        assert!(!overlay.should_show(PhysicalPosition::new(10.0, 60.0)));
    }

    #[test]
    fn overlay_moves_but_stays_on_screen() {
        let size = PhysicalSize::new(1920, 1080);

        // Утащили влево до упора: шторка прижата к краю, но не за ним.
        let overlay = Overlay::new(size, 1.0, Overlay::center_for(size, 1.0, 1, -500.0), 1);
        assert_eq!(overlay.bar.x, 0.0);

        // Вправо до упора — правый край шторки совпадает с краем окна.
        let overlay = Overlay::new(size, 1.0, Overlay::center_for(size, 1.0, 1, 5000.0), 1);
        assert!((overlay.bar.x + overlay.bar.w - 1920.0).abs() < 0.5);

        // Куда поставили, там и стоит.
        let overlay = Overlay::new(size, 1.0, Overlay::center_for(size, 1.0, 1, 100.0), 1);
        assert!((overlay.bar.x - 100.0).abs() < 0.5);

        // Кнопки ручкой не считаются — иначе их нельзя было бы нажать.
        let hit = overlay.disconnect;
        assert!(!overlay.is_handle((hit.x + 2.0) as f64, (hit.y + 2.0) as f64));
        assert!(overlay.is_handle((overlay.bar.x + 10.0) as f64, 10.0));
    }

    #[test]
    fn monitor_buttons_fit_before_window_buttons() {
        // Один монитор — переключать нечего, кнопок нет.
        let single = Overlay::new(PhysicalSize::new(1920, 1080), 1.0, 0.5, 1);
        assert!(single.monitors.is_empty());

        for count in [2, 3, 4] {
            let overlay = Overlay::new(PhysicalSize::new(1920, 1080), 1.0, 0.5, count);
            assert_eq!(overlay.monitors.len(), count);
            // Все кнопки внутри шторки, правее надписи с адресом и левее «свернуть».
            for hit in &overlay.monitors {
                assert!(hit.x >= overlay.bar.x + 220.0, "кнопка наехала на адрес");
                assert!(hit.x + hit.w <= overlay.minimize.x);
            }
            // Каждый номер узнаётся своим.
            for (i, hit) in overlay.monitors.iter().enumerate() {
                let center = ((hit.x + hit.w / 2.0) as f64, (hit.y + hit.h / 2.0) as f64);
                assert_eq!(overlay.button_at(center.0, center.1), Some(BarButton::Monitor(i)));
            }
        }
    }

    #[test]
    fn frame_fills_the_window() {
        // Окно шире кадра: картинка обязана растянуться, а не оставить поля.
        let placement = Placement::new(PhysicalSize::new(1920, 1080), 1366, 768);
        assert!(placement.draw_w > 1366);
        assert!(placement.draw_w <= 1920 && placement.draw_h <= 1080);
    }
}

/// Скриншот шторки для README: `cargo test -p skydesktop-app -- --ignored screenshots`.
#[cfg(test)]
mod screenshots {
    use super::*;

    #[test]
    #[ignore = "пишет файлы в docs/screenshots"]
    fn screenshots() {
        let fonts = crate::paint::Fonts::load().expect("шрифты вшиты в exe");
        let scale = 1.5;
        let overlay = Overlay::new(PhysicalSize::new(1920, 1080), scale, 0.5, 2);
        let state = BarState {
            overlay: &overlay,
            hovered: None,
            current_display: 0,
            fullscreen: true,
        };
        let pixmap = bar_pixmap(&fonts, &state, "123456789").unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots");
        std::fs::create_dir_all(&dir).unwrap();
        pixmap.save_png(dir.join("toolbar.png")).unwrap();
    }
}
