//! Отрисовка интерфейса своими силами.
//!
//! Всё рисуется на процессоре и выводится через `softbuffer`: не нужен ни
//! WebView2, ни драйверы OpenGL/Direct3D, поэтому окна одинаково работают
//! и на свежей Windows, и на урезанной сборке в виртуальной машине.

use std::path::Path;

use fontdue::{Font, FontSettings};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect, Transform};

/// Фирменные цвета — те же, что в оформлении приложения.
///
/// Функциями, а не константами: конструктор цвета в `tiny-skia` не константный.
pub mod palette {
    use tiny_skia::Color;

    pub fn brand() -> Color {
        rgb(0x12, 0x68, 0xdd)
    }

    pub fn brand_hover() -> Color {
        rgb(0x0d, 0x55, 0xb8)
    }

    pub fn surface() -> Color {
        rgb(0xff, 0xff, 0xff)
    }

    pub fn border() -> Color {
        rgb(0xd8, 0xda, 0xdc)
    }

    pub fn text() -> Color {
        rgb(0x33, 0x36, 0x3a)
    }

    pub fn text_muted() -> Color {
        rgb(0x6e, 0x72, 0x76)
    }

    pub fn hover_fill() -> Color {
        rgb(0xee, 0xf0, 0xf2)
    }

    /// Фон рабочей области — как в макете.
    pub fn canvas() -> Color {
        rgb(0xed, 0xef, 0xf1)
    }

    /// Подсветка выбранного пункта меню и аватаров.
    pub fn brand_soft() -> Color {
        rgb(0xe8, 0xf1, 0xfe)
    }

    /// Разделители внутри карточек.
    pub fn divider() -> Color {
        rgb(0xec, 0xee, 0xf0)
    }

    /// Заголовки в шапке таблицы.
    pub fn text_faint() -> Color {
        rgb(0x9a, 0xa0, 0xa6)
    }

    /// Подложка поля ввода и шапки таблицы.
    pub fn field() -> Color {
        Color::from_rgba8(0xf7, 0xf9, 0xfb, 255)
    }

    /// Мягкая тень под карточкой.
    pub fn shadow() -> Color {
        Color::from_rgba8(0xe1, 0xe6, 0xec, 255)
    }

    /// Янтарный — сервер отвечает, но адрес за нами не закреплён.
    pub fn warning() -> Color {
        Color::from_rgba8(0xd9, 0x77, 0x06, 255)
    }

    /// Бледный зелёный — подложка под значком «в сети».
    pub fn success_soft() -> Color {
        Color::from_rgba8(0xe4, 0xf5, 0xe9, 255)
    }

    pub fn success() -> Color {
        rgb(0x2e, 0xae, 0x5c)
    }

    fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color::from_rgba8(r, g, b, 255)
    }
}

/// Шрифты интерфейса: обычный и полужирный.
pub struct Fonts {
    regular: Font,
    bold: Font,
}

/// Roboto вшит в exe: интерфейс выглядит одинаково на любой машине,
/// не зависит от набора системных шрифтов и покрывает кириллицу.
const ROBOTO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Roboto-Regular.ttf");
const ROBOTO_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Roboto-Medium.ttf");

impl Fonts {
    /// Загружает вшитый Roboto; системные шрифты — запасной вариант.
    pub fn load() -> Option<Self> {
        let embedded = |bytes: &[u8]| Font::from_bytes(bytes.to_vec(), FontSettings::default()).ok();

        let regular = embedded(ROBOTO_REGULAR)
            .or_else(|| load_first(&["segoeui.ttf", "tahoma.ttf", "arial.ttf"]))?;
        // Полужирного может не оказаться — тогда используем обычный.
        let bold = embedded(ROBOTO_MEDIUM)
            .or_else(|| load_first(&["segoeuib.ttf", "tahomabd.ttf", "arialbd.ttf"]))
            .unwrap_or_else(|| regular.clone());
        Some(Self { regular, bold })
    }

    fn font(&self, bold: bool) -> &Font {
        if bold {
            &self.bold
        } else {
            &self.regular
        }
    }

    /// Ширина строки в пикселях.
    pub fn width(&self, text: &str, size: f32, bold: bool) -> f32 {
        let font = self.font(bold);
        text.chars()
            .map(|ch| font.metrics(ch, size).advance_width)
            .sum()
    }

    /// Рисует строку, `x`/`y` — левый край и базовая линия.
    pub fn draw(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        bold: bool,
        color: Color,
    ) {
        let font = self.font(bold);
        let mut pen = x;

        for ch in text.chars() {
            let (metrics, bitmap) = font.rasterize(ch, size);
            if metrics.width > 0 && metrics.height > 0 {
                blend_glyph(
                    pixmap,
                    &bitmap,
                    metrics.width,
                    metrics.height,
                    (pen + metrics.xmin as f32).round() as i32,
                    (y - (metrics.height as i32 + metrics.ymin) as f32).round() as i32,
                    color,
                );
            }
            pen += metrics.advance_width;
        }
    }
}

impl Clone for Fonts {
    fn clone(&self) -> Self {
        Self {
            regular: self.regular.clone(),
            bold: self.bold.clone(),
        }
    }
}

fn load_first(names: &[&str]) -> Option<Font> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
    for name in names {
        let path = Path::new(&dir).join("Fonts").join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                return Some(font);
            }
        }
    }
    None
}

/// Накладывает растр глифа с учётом покрытия.
fn blend_glyph(
    pixmap: &mut Pixmap,
    coverage: &[u8],
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    color: Color,
) {
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    let data = pixmap.pixels_mut();
    let (r, g, b) = (color.red(), color.green(), color.blue());

    for row in 0..height as i32 {
        let y = top + row;
        if y < 0 || y >= ph {
            continue;
        }
        for col in 0..width as i32 {
            let x = left + col;
            if x < 0 || x >= pw {
                continue;
            }
            let alpha = coverage[row as usize * width + col as usize] as f32 / 255.0;
            if alpha <= 0.0 {
                continue;
            }
            let index = (y * pw + x) as usize;
            let dst = data[index];
            let mix = |src: f32, dst: u8| -> u8 {
                ((src * alpha + dst as f32 / 255.0 * (1.0 - alpha)) * 255.0).round() as u8
            };
            data[index] = tiny_skia::PremultipliedColorU8::from_rgba(
                mix(r, dst.red()),
                mix(g, dst.green()),
                mix(b, dst.blue()),
                255,
            )
            .unwrap_or(dst);
        }
    }
}

/// Прямоугольник со скруглением.
pub fn rounded_rect(pixmap: &mut Pixmap, rect: Rect, radius: f32, color: Color, stroke: bool) {
    let (x, y, w, h) = (rect.x(), rect.y(), rect.width(), rect.height());
    let r = radius.min(w / 2.0).min(h / 2.0);

    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.quad_to(x + w, y, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.quad_to(x + w, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.quad_to(x, y + h, x, y + h - r);
    path.line_to(x, y + r);
    path.quad_to(x, y, x + r, y);
    path.close();

    let Some(path) = path.finish() else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    if stroke {
        let mut props = tiny_skia::Stroke::default();
        props.width = 1.0;
        pixmap.stroke_path(&path, &paint, &props, Transform::identity(), None);
    } else {
        pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Укорачивает строку до заданной ширины, добавляя многоточие.
pub fn ellipsize(fonts: &Fonts, text: &str, size: f32, bold: bool, max_width: f32) -> String {
    if fonts.width(text, size, bold) <= max_width {
        return text.to_string();
    }
    let mut cut: String = text.to_string();
    while !cut.is_empty() && fonts.width(&format!("{cut}…"), size, bold) > max_width {
        cut.pop();
    }
    format!("{cut}…")
}

/// Рисует картинку, вписывая её в квадрат со стороной `size`.
///
/// Прямой блит с ближайшим соседом: масштаб фиксирован и невелик, а через
/// шейдер `tiny-skia` пришлось бы возиться с преобразованиями узора.
pub fn draw_image(pixmap: &mut Pixmap, image: &Pixmap, left: f32, top: f32, size: f32) {
    let (dw, dh) = (size.round() as i32, size.round() as i32);
    let (sw, sh) = (image.width() as i32, image.height() as i32);
    if dw <= 0 || dh <= 0 || sw == 0 || sh == 0 {
        return;
    }
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    let (left, top) = (left.round() as i32, top.round() as i32);
    let source = image.pixels().to_vec();
    let data = pixmap.pixels_mut();

    for row in 0..dh {
        let y = top + row;
        if y < 0 || y >= ph {
            continue;
        }
        // Каждой точке назначения соответствует прямоугольник исходника.
        // Логотип 256×256 рисуется размером в 42 точки, поэтому брать один
        // пиксель из тридцати шести нельзя — усредняем весь прямоугольник.
        let y0 = row * sh / dh;
        let y1 = (((row + 1) * sh + dh - 1) / dh).max(y0 + 1).min(sh);

        for col in 0..dw {
            let x = left + col;
            if x < 0 || x >= pw {
                continue;
            }
            let x0 = col * sw / dw;
            let x1 = (((col + 1) * sw + dw - 1) / dw).max(x0 + 1).min(sw);

            // Цвета премультиплицированы, поэтому усреднять их можно прямо так.
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let src = source[(sy * sw + sx) as usize];
                    r += src.red() as u32;
                    g += src.green() as u32;
                    b += src.blue() as u32;
                    a += src.alpha() as u32;
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            if count == 0 || a == 0 {
                continue;
            }
            let (sr, sg, sb) = ((r / count) as u8, (g / count) as u8, (b / count) as u8);
            let alpha = (a / count) as f32 / 255.0;

            let index = (y * pw + x) as usize;
            let dst = data[index];
            // Источник премультиплицирован, поэтому доля фона — (1 - alpha).
            let mix = |s: u8, d: u8| -> u8 {
                (s as f32 + d as f32 * (1.0 - alpha)).round().min(255.0) as u8
            };
            data[index] = tiny_skia::PremultipliedColorU8::from_rgba(
                mix(sr, dst.red()),
                mix(sg, dst.green()),
                mix(sb, dst.blue()),
                255,
            )
            .unwrap_or(dst);
        }
    }
}

/// Переносит нарисованное в буфер окна.
///
/// `softbuffer` ждёт `0RGB`, а `tiny-skia` хранит премультиплицированный RGBA;
/// фон непрозрачный, поэтому достаточно переставить каналы.
pub fn present(pixmap: &Pixmap, buffer: &mut [u32]) {
    for (dst, src) in buffer.iter_mut().zip(pixmap.pixels()) {
        *dst = ((src.red() as u32) << 16) | ((src.green() as u32) << 8) | src.blue() as u32;
    }
}

/// Прямоугольник в экранных координатах — для попадания мышью.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hit {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Hit {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (x, y) = (x as f32, y as f32);
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}
