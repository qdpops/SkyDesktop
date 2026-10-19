//! Значки интерфейса — Bootstrap Icons 1.11.3.
//!
//! Каждый значок лежит в `assets/icons` как исходный SVG и вшивается в exe.
//! Рисуем их сами: разбираем атрибут `d` и складываем путь `tiny_skia`.
//! Один источник на все окна — поэтому толщина линий и скругления везде
//! одинаковые, чего не добиться от значков, нарисованных вручную.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Transform};

/// Сетка, в которой нарисованы все значки Bootstrap.
const GRID: f32 = 16.0;

/// Тип функции рисования — им параметризуются вызовы из панели.
pub type Draw = fn(&mut Pixmap, f32, f32, f32, Color);

/// Значок «спрятать пароль»; парный к [`eye`].
pub use eye_slash as hidden;

macro_rules! icons {
    ($($name:ident => $file:literal),* $(,)?) => {
        $(
            #[doc = concat!("Bootstrap Icons: `", $file, "`.")]
            pub fn $name(pixmap: &mut Pixmap, x: f32, y: f32, size: f32, color: Color) {
                draw(
                    pixmap,
                    include_str!(concat!("../../../assets/icons/", $file, ".svg")),
                    x,
                    y,
                    size,
                    color,
                );
            }
        )*
    };
}

icons! {
    monitor => "display",
    gear => "gear",
    signal => "broadcast-pin",
    wifi => "wifi",
    clock => "clock-history",
    bolt => "lightning-charge-fill",
    trash => "trash3",
    minimize => "dash-lg",
    maximize => "square",
    close => "x-lg",
    eye => "eye",
    eye_slash => "eye-slash",
    key => "key",
    stop => "x-circle",
    check => "check-lg",
    windowed => "fullscreen-exit",
    disconnect => "box-arrow-right",
    shield => "shield-lock-fill",
    download => "download",
    refresh => "arrow-clockwise",
}

/// Рисует значок в квадрате `size` с левым верхним углом в (x, y).
fn draw(pixmap: &mut Pixmap, source: &'static str, x: f32, y: f32, size: f32, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    let scale = size / GRID;
    let transform = Transform::from_row(scale, 0.0, 0.0, scale, x, y);

    for (path, rule) in shapes(source) {
        pixmap.fill_path(&path, &paint, rule, transform, None);
    }
}

/// Разобранные пути значка. Разбор идёт один раз на значок за запуск.
fn shapes(source: &'static str) -> Vec<(Path, FillRule)> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Vec<(Path, FillRule)>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    // Ключ — адрес вшитой строки: у каждого значка он свой и не меняется.
    let key = source.as_ptr() as usize;
    let mut cache = cache.lock().expect("кэш значков");
    cache.entry(key).or_insert_with(|| parse(source)).clone()
}

/// Достаёт из SVG все `<path>` вместе с правилом заливки.
fn parse(source: &str) -> Vec<(Path, FillRule)> {
    let mut shapes = Vec::new();

    for tag in source.split("<path").skip(1) {
        let Some(tag) = tag.split('>').next() else {
            continue;
        };
        let Some(data) = attribute(tag, "d") else {
            continue;
        };
        let rule = match attribute(tag, "fill-rule").as_deref() {
            Some("evenodd") => FillRule::EvenOdd,
            _ => FillRule::Winding,
        };
        if let Some(path) = build(&data) {
            shapes.push((path, rule));
        }
    }

    shapes
}

/// Значение атрибута в двойных кавычках.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Складывает путь по данным атрибута `d`.
fn build(data: &str) -> Option<Path> {
    use svgtypes::PathSegment as Segment;

    let mut builder = PathBuilder::new();
    // Текущая точка, начало подпути и последние контрольные точки:
    // они нужны сокращённым командам `s` и `t`.
    let (mut px, mut py) = (0.0_f32, 0.0_f32);
    let (mut sx, mut sy) = (0.0_f32, 0.0_f32);
    let mut last_cubic: Option<(f32, f32)> = None;
    let mut last_quad: Option<(f32, f32)> = None;

    for segment in svgtypes::PathParser::from(data) {
        let Ok(segment) = segment else { return None };

        // Относительные команды отсчитываются от текущей точки.
        let shift = |abs: bool, x: f64, y: f64| {
            if abs {
                (x as f32, y as f32)
            } else {
                (px + x as f32, py + y as f32)
            }
        };

        match segment {
            Segment::MoveTo { abs, x, y } => {
                let (x, y) = shift(abs, x, y);
                builder.move_to(x, y);
                (px, py) = (x, y);
                (sx, sy) = (x, y);
                (last_cubic, last_quad) = (None, None);
            }
            Segment::LineTo { abs, x, y } => {
                let (x, y) = shift(abs, x, y);
                builder.line_to(x, y);
                (px, py) = (x, y);
                (last_cubic, last_quad) = (None, None);
            }
            Segment::HorizontalLineTo { abs, x } => {
                let x = if abs { x as f32 } else { px + x as f32 };
                builder.line_to(x, py);
                px = x;
                (last_cubic, last_quad) = (None, None);
            }
            Segment::VerticalLineTo { abs, y } => {
                let y = if abs { y as f32 } else { py + y as f32 };
                builder.line_to(px, y);
                py = y;
                (last_cubic, last_quad) = (None, None);
            }
            Segment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (x1, y1) = shift(abs, x1, y1);
                let (x2, y2) = shift(abs, x2, y2);
                let (x, y) = shift(abs, x, y);
                builder.cubic_to(x1, y1, x2, y2, x, y);
                (px, py) = (x, y);
                last_cubic = Some((x2, y2));
                last_quad = None;
            }
            Segment::SmoothCurveTo { abs, x2, y2, x, y } => {
                // Первая контрольная точка — отражение предыдущей.
                let (rx, ry) = last_cubic.unwrap_or((px, py));
                let (x1, y1) = (2.0 * px - rx, 2.0 * py - ry);
                let (x2, y2) = shift(abs, x2, y2);
                let (x, y) = shift(abs, x, y);
                builder.cubic_to(x1, y1, x2, y2, x, y);
                (px, py) = (x, y);
                last_cubic = Some((x2, y2));
                last_quad = None;
            }
            Segment::Quadratic { abs, x1, y1, x, y } => {
                let (x1, y1) = shift(abs, x1, y1);
                let (x, y) = shift(abs, x, y);
                builder.quad_to(x1, y1, x, y);
                (px, py) = (x, y);
                last_quad = Some((x1, y1));
                last_cubic = None;
            }
            Segment::SmoothQuadratic { abs, x, y } => {
                let (rx, ry) = last_quad.unwrap_or((px, py));
                let (x1, y1) = (2.0 * px - rx, 2.0 * py - ry);
                let (x, y) = shift(abs, x, y);
                builder.quad_to(x1, y1, x, y);
                (px, py) = (x, y);
                last_quad = Some((x1, y1));
                last_cubic = None;
            }
            Segment::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                let (ex, ey) = shift(abs, x, y);
                arc_to(
                    &mut builder,
                    (px, py),
                    (rx as f32, ry as f32),
                    x_axis_rotation as f32,
                    large_arc,
                    sweep,
                    (ex, ey),
                );
                (px, py) = (ex, ey);
                (last_cubic, last_quad) = (None, None);
            }
            Segment::ClosePath { .. } => {
                builder.close();
                (px, py) = (sx, sy);
                (last_cubic, last_quad) = (None, None);
            }
        }
    }

    builder.finish()
}

/// Дуга эллипса кубическими кривыми — по приложению F.6 спецификации SVG.
fn arc_to(
    builder: &mut PathBuilder,
    from: (f32, f32),
    radii: (f32, f32),
    rotation_deg: f32,
    large_arc: bool,
    sweep: bool,
    to: (f32, f32),
) {
    let (x1, y1) = from;
    let (x2, y2) = to;
    let (mut rx, mut ry) = (radii.0.abs(), radii.1.abs());

    // Вырожденная дуга — обычный отрезок.
    if rx < f32::EPSILON || ry < f32::EPSILON || (x1 - x2).abs() + (y1 - y2).abs() < f32::EPSILON {
        builder.line_to(x2, y2);
        return;
    }

    let phi = rotation_deg.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();

    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;

    // Радиусы могут не дотягиваться до конечной точки — тогда их растягивают.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }

    let numerator = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
    let denominator = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let coefficient = if denominator < f32::EPSILON {
        0.0
    } else {
        let sign = if large_arc == sweep { -1.0 } else { 1.0 };
        sign * (numerator / denominator).sqrt()
    };

    let cxp = coefficient * rx * y1p / ry;
    let cyp = -coefficient * ry * x1p / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (x1 + x2) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (y1 + y2) / 2.0;

    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut value = if len < f32::EPSILON {
            0.0
        } else {
            (dot / len).clamp(-1.0, 1.0).acos()
        };
        if ux * vy - uy * vx < 0.0 {
            value = -value;
        }
        value
    };

    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;

    let start = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    let two_pi = std::f32::consts::PI * 2.0;
    if !sweep && delta > 0.0 {
        delta -= two_pi;
    } else if sweep && delta < 0.0 {
        delta += two_pi;
    }

    // Дугу режем на куски не длиннее четверти оборота: так кубика точна.
    let steps = (delta.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / steps as f32;
    let alpha = (4.0 / 3.0) * (step / 4.0).tan();

    let mut theta = start;
    for _ in 0..steps {
        let (sin1, cos1) = theta.sin_cos();
        let (sin2, cos2) = (theta + step).sin_cos();

        let point = |cos: f32, sin: f32| {
            (
                cx + rx * cos * cos_phi - ry * sin * sin_phi,
                cy + rx * cos * sin_phi + ry * sin * cos_phi,
            )
        };
        let tangent = |cos: f32, sin: f32| {
            (
                -rx * sin * cos_phi - ry * cos * sin_phi,
                -rx * sin * sin_phi + ry * cos * cos_phi,
            )
        };

        let (sx, sy) = point(cos1, sin1);
        let (ex, ey) = point(cos2, sin2);
        let (dsx, dsy) = tangent(cos1, sin1);
        let (dex, dey) = tangent(cos2, sin2);

        builder.cubic_to(
            sx + alpha * dsx,
            sy + alpha * dsy,
            ex - alpha * dex,
            ey - alpha * dey,
            ex,
            ey,
        );
        theta += step;
    }
}
