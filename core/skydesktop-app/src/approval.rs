//! Плашка запроса доступа.
//!
//! Появляется, когда кто-то подключается к этому компьютеру и решение за
//! человеком. Узкая полоса поверх остальных окон: кто подключается, «Принять»,
//! «Отклонить» и закрытие. Рисуется своими силами — WebView здесь не нужен.
//!
//! Процесс запускает само ядро (`--cm`) и держит его, пока идут сеансы,
//! поэтому закрытие плашки её только прячет: она вернётся к следующему запросу.

use std::num::NonZeroU32;
use std::rc::Rc;

use skydesktop_bridge::{CoreEvent, IncomingClientDto};
use tiny_skia::{Pixmap, Rect};

use tao::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Event, MouseButton, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget},
    monitor::MonitorHandle,
    window::{Window, WindowBuilder},
};

use crate::engine::{self, EventBus, Request};
use crate::paint::{self, palette, Fonts, Hit};

/// Логический размер плашки.
const WIDTH: f64 = 520.0;
const HEIGHT: f64 = 96.0;

/// Отступ от верхнего края экрана.
const TOP_MARGIN: f64 = 28.0;

/// Что произошло за пределами цикла окна.
#[derive(Debug, Clone)]
enum Incoming {
    /// Кто-то ждёт разрешения.
    Waiting(IncomingClientDto),
    /// Ожидающих больше нет.
    Gone,
}

/// Какая часть плашки под курсором.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    None,
    Accept,
    Decline,
    Close,
}

/// Открывает окно запроса доступа и не возвращает управление до его закрытия.
pub fn run() -> ! {
    let event_loop = EventLoopBuilder::<Incoming>::with_user_event().build();

    let bus = EventBus::new();
    let engine = engine::start(bus.clone());

    // Подписка строго до запуска менеджера: шина доставляет события только
    // тем, кто уже подписан, а первый запрос приходит почти сразу.
    {
        let proxy = event_loop.create_proxy();
        let mut events = bus.subscribe();
        std::thread::Builder::new()
            .name("skydesktop-approval".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("не удалось создать рантайм");
                runtime.block_on(async move {
                    while let Ok(event) = events.recv().await {
                        let message = match event {
                            CoreEvent::IncomingConnection { client } if !client.authorized => {
                                Some(Incoming::Waiting(client))
                            }
                            // Разрешённое подключение решения уже не требует.
                            CoreEvent::IncomingConnection { .. }
                            | CoreEvent::ConnectionClosed { .. } => Some(Incoming::Gone),
                            _ => None,
                        };
                        if let Some(message) = message {
                            let _ = proxy.send_event(message);
                        }
                    }
                });
            })
            .expect("не удалось запустить приём событий");
    }

    // Панель не может закрыть подключение сама: реестр живёт здесь. Поднимаем
    // маленькую точку управления и сообщаем её адрес вместе со списком.
    let control = start_control(engine.clone());
    engine::start_connection_manager(bus.clone(), control);

    let fonts = Fonts::load();
    let scale = primary_scale(&event_loop);
    let size = PhysicalSize::new(
        (WIDTH * scale).round() as u32,
        (HEIGHT * scale).round() as u32,
    );

    let window = Rc::new(
        WindowBuilder::new()
            .with_title("SkyDesktop — запрос доступа")
            .with_inner_size(size)
            .with_position(top_center(&event_loop, size))
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top(true)
            .with_visible(false)
            .with_window_icon(crate::viewer::app_icon())
            .build(&event_loop)
            .expect("не удалось создать окно запроса доступа"),
    );

    let context = softbuffer::Context::new(window.clone()).expect("нет графического контекста");
    let mut surface =
        softbuffer::Surface::new(&context, window.clone()).expect("не удалось создать поверхность");

    let mut waiting: Option<IncomingClientDto> = None;
    let mut cursor = PhysicalPosition::new(-1.0f64, -1.0f64);
    let mut hovered = Zone::None;

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(Incoming::Waiting(client)) => {
                tracing::info!("плашка: показываю запрос от {}", client.peer_id);
                waiting = Some(client);
                window.set_visible(true);
                window.set_always_on_top(true);
                bring_to_front(&window);
                window.request_redraw();
            }
            Event::UserEvent(Incoming::Gone) => {
                waiting = None;
                window.set_visible(false);
            }

            Event::RedrawRequested(_) => {
                if let Some(client) = &waiting {
                    draw(&window, &mut surface, fonts.as_ref(), client, hovered, scale);
                }
            }

            Event::WindowEvent { event, .. } => match event {
                // Плашка держит установленные сеансы: закрытие её прячет.
                WindowEvent::CloseRequested => window.set_visible(false),

                WindowEvent::CursorMoved { position, .. } => {
                    cursor = position;
                    let zone = zone_at(&window, position, scale);
                    if zone != hovered {
                        hovered = zone;
                        window.request_redraw();
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    if hovered != Zone::None {
                        hovered = Zone::None;
                        window.request_redraw();
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => match zone_at(&window, cursor, scale) {
                    Zone::Accept => {
                        if let Some(client) = waiting.take() {
                            engine.send(Request::Authorize(client.conn_id));
                        }
                        window.set_visible(false);
                    }
                    Zone::Decline => {
                        if let Some(client) = waiting.take() {
                            engine.send(Request::CloseConnection(client.conn_id));
                        }
                        window.set_visible(false);
                    }
                    Zone::Close => window.set_visible(false),
                    // Тянем окно за свободное место плашки.
                    Zone::None => {
                        let _ = window.drag_window();
                    }
                },
                _ => {}
            },

            _ => {}
        }
    })
}

/// Раскладка плашки в логических единицах.
struct Layout {
    accept: Hit,
    decline: Hit,
    close: Hit,
}

impl Layout {
    fn new(width: f32) -> Self {
        let close = Hit {
            x: width - 30.0,
            y: 8.0,
            w: 22.0,
            h: 22.0,
        };
        let accept = Hit {
            x: width - 132.0,
            y: 52.0,
            w: 112.0,
            h: 32.0,
        };
        let decline = Hit {
            x: width - 246.0,
            y: 52.0,
            w: 104.0,
            h: 32.0,
        };
        Self {
            accept,
            decline,
            close,
        }
    }
}

fn zone_at(window: &Window, position: PhysicalPosition<f64>, scale: f64) -> Zone {
    let layout = Layout::new(window.inner_size().width as f32 / scale as f32);
    let (x, y) = (position.x / scale, position.y / scale);

    if layout.close.contains(x, y) {
        Zone::Close
    } else if layout.accept.contains(x, y) {
        Zone::Accept
    } else if layout.decline.contains(x, y) {
        Zone::Decline
    } else {
        Zone::None
    }
}

fn draw(
    window: &Window,
    surface: &mut softbuffer::Surface<Rc<Window>, Rc<Window>>,
    fonts: Option<&Fonts>,
    client: &IncomingClientDto,
    hovered: Zone,
    scale: f64,
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
    let Some(mut pixmap) = Pixmap::new(size.width, size.height) else {
        return;
    };

    let scale = scale as f32;
    let w = size.width as f32 / scale;
    let h = size.height as f32 / scale;
    let layout = Layout::new(w);
    let px = |value: f32| value * scale;

    pixmap.fill(palette::surface());

    // Рамка плашки.
    if let Some(rect) = Rect::from_xywh(px(0.5), px(0.5), px(w - 1.0), px(h - 1.0)) {
        paint::rounded_rect(&mut pixmap, rect, px(10.0), palette::border(), true);
    }

    // Замок слева: сразу видно, что плашка про доступ к компьютеру.
    crate::icons::shield(&mut pixmap, px(20.0), px(34.0), px(26.0), palette::brand());

    let Some(fonts) = fonts else {
        paint::present(&pixmap, &mut buffer);
        let _ = buffer.present();
        return;
    };

    let name = if client.name.is_empty() {
        client.peer_id.clone()
    } else {
        client.name.clone()
    };

    fonts.draw(
        &mut pixmap,
        "Запрос доступа к этому компьютеру",
        px(58.0),
        px(38.0),
        px(12.5),
        false,
        palette::text_muted(),
    );
    fonts.draw(
        &mut pixmap,
        &format!("{name} · {}", client.peer_id),
        px(58.0),
        px(62.0),
        px(15.0),
        true,
        palette::text(),
    );

    button(
        &mut pixmap,
        fonts,
        layout.accept,
        "Принять",
        crate::icons::check,
        true,
        hovered == Zone::Accept,
        scale,
    );
    button(
        &mut pixmap,
        fonts,
        layout.decline,
        "Отклонить",
        crate::icons::stop,
        false,
        hovered == Zone::Decline,
        scale,
    );

    // Крестик закрытия.
    if hovered == Zone::Close {
        if let Some(rect) = Rect::from_xywh(
            px(layout.close.x),
            px(layout.close.y),
            px(layout.close.w),
            px(layout.close.h),
        ) {
            paint::rounded_rect(&mut pixmap, rect, px(5.0), palette::hover_fill(), false);
        }
    }
    let cross = px(11.0);
    crate::icons::close(
        &mut pixmap,
        px(layout.close.x + layout.close.w / 2.0) - cross / 2.0,
        px(layout.close.y + layout.close.h / 2.0) - cross / 2.0,
        cross,
        palette::text_muted(),
    );

    paint::present(&pixmap, &mut buffer);
    let _ = buffer.present();
}

fn button(
    pixmap: &mut Pixmap,
    fonts: &Fonts,
    hit: Hit,
    label: &str,
    icon: crate::icons::Draw,
    primary: bool,
    hovered: bool,
    scale: f32,
) {
    let px = |value: f32| value * scale;
    let Some(rect) = Rect::from_xywh(px(hit.x), px(hit.y), px(hit.w), px(hit.h)) else {
        return;
    };

    let (fill, text) = if primary {
        (
            if hovered {
                palette::brand_hover()
            } else {
                palette::brand()
            },
            palette::surface(),
        )
    } else {
        (
            if hovered {
                palette::hover_fill()
            } else {
                palette::surface()
            },
            palette::text(),
        )
    };

    paint::rounded_rect(pixmap, rect, px(7.0), fill, false);
    if !primary {
        paint::rounded_rect(pixmap, rect, px(7.0), palette::border(), true);
    }

    // Значок и подпись идут вместе и центрируются как одно целое.
    let size = px(13.0);
    let glyph = px(14.0);
    let gap = px(7.0);
    let width = fonts.width(label, size, true);
    let start = px(hit.x + hit.w / 2.0) - (width + glyph + gap) / 2.0;

    icon(
        pixmap,
        start,
        px(hit.y + hit.h / 2.0) - glyph / 2.0,
        glyph,
        text,
    );
    fonts.draw(
        pixmap,
        label,
        start + glyph + gap,
        px(hit.y + hit.h / 2.0) + size * 0.36,
        size,
        true,
        text,
    );
}

fn primary_scale(target: &EventLoopWindowTarget<Incoming>) -> f64 {
    target
        .primary_monitor()
        .or_else(|| target.available_monitors().next())
        .map(|m| m.scale_factor())
        .unwrap_or(1.0)
}

/// Плашка встаёт сверху по центру экрана — как уведомление.
fn top_center(
    target: &EventLoopWindowTarget<Incoming>,
    size: PhysicalSize<u32>,
) -> PhysicalPosition<i32> {
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
    let scale = monitor
        .as_ref()
        .map(MonitorHandle::scale_factor)
        .unwrap_or(1.0);

    PhysicalPosition::new(
        origin.x + (screen.width.saturating_sub(size.width) / 2) as i32,
        origin.y + (TOP_MARGIN * scale) as i32,
    )
}

/// Windows не отдаёт фокус фоновому процессу сам: плашку поднимаем явно.
#[cfg(windows)]
fn bring_to_front(window: &Window) {
    use tao::platform::windows::WindowExtWindows;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, SetWindowPos, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE,
    };

    let hwnd = window.hwnd() as windows_sys::Win32::Foundation::HWND;
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        SetForegroundWindow(hwnd);
    }
}

#[cfg(not(windows))]
fn bring_to_front(_window: &Window) {}

/// Поднимает точку управления и возвращает её адрес.
fn start_control(engine: engine::Engine) -> String {
    use axum::{extract::State, routing::post, Json, Router};

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Disconnect {
        conn_id: i32,
    }

    let runtime = tokio::runtime::Runtime::new().expect("не удалось создать рантайм");
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(("127.0.0.1", 0)))
        .expect("не удалось занять локальный порт");
    let address = listener
        .local_addr()
        .expect("у слушателя есть адрес")
        .to_string();

    let router = Router::new()
        .route(
            "/disconnect",
            post(
                |State(engine): State<engine::Engine>, Json(request): Json<Disconnect>| async move {
                    tracing::info!("менеджер: разорвать подключение {}", request.conn_id);
                    engine.send(Request::CloseConnection(request.conn_id));
                    axum::http::StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(engine);

    runtime.spawn(async move {
        if let Err(err) = axum::serve(listener, router).await {
            tracing::error!("точка управления остановлена: {err}");
        }
    });
    // Рантайм должен пережить функцию: сервер живёт до конца процесса.
    std::mem::forget(runtime);

    format!("http://{address}")
}
