//! Small vector charts drawn on a canvas: a ring for "how full" and an area line for history.

use gpui_kit::*;

fn polar(c: Point<Pixels>, r: f32, turn: f32) -> Point<Pixels> {
    // 0 at twelve o'clock, clockwise
    let a = (turn - 0.25) * std::f32::consts::TAU;
    point(c.x + px(r * a.cos()), c.y + px(r * a.sin()))
}

fn arc(c: Point<Pixels>, r: f32, from: f32, to: f32, width: f32) -> Option<Path<Pixels>> {
    let steps = (((to - from) * 160.0).ceil() as usize).max(2);
    let mut b = PathBuilder::stroke(px(width));
    b.move_to(polar(c, r, from));
    for i in 1..=steps {
        b.line_to(polar(c, r, from + (to - from) * i as f32 / steps as f32));
    }
    b.build().ok()
}

fn dot(c: Point<Pixels>, r: f32) -> Option<Path<Pixels>> {
    let mut b = PathBuilder::fill();
    let steps = 24;
    b.move_to(point(c.x + px(r), c.y));
    for i in 1..=steps {
        let a = i as f32 / steps as f32 * std::f32::consts::TAU;
        b.line_to(point(c.x + px(r * a.cos()), c.y + px(r * a.sin())));
    }
    b.close();
    b.build().ok()
}

/// A ring filled to `fraction`, with rounded ends; `size` is the outer diameter.
pub fn ring(fraction: f32, color: Hsla, track: Hsla, size: f32, thickness: f32) -> impl IntoElement {
    let fraction = fraction.clamp(0.0, 1.0);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let c = bounds.center();
            let r = size / 2.0 - thickness / 2.0;
            if let Some(p) = arc(c, r, 0.0, 1.0, thickness) {
                window.paint_path(p, track);
            }
            if fraction > 0.002 {
                if let Some(p) = arc(c, r, 0.0, fraction, thickness) {
                    window.paint_path(p, color);
                }
                // round caps
                for t in [0.0, fraction] {
                    if let Some(p) = dot(polar(c, r, t), thickness / 2.0) {
                        window.paint_path(p, color);
                    }
                }
            }
        },
    )
    .size(px(size))
    .flex_shrink_0()
}

/// Values over time as a smooth line with a fading fill underneath.
pub fn area(values: Vec<f64>, color: Hsla, height: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if values.len() < 2 {
                return;
            }
            let max = values.iter().cloned().fold(f64::MIN, f64::max);
            let min = values.iter().cloned().fold(f64::MAX, f64::min);
            let span = (max - min).max(max.abs() * 0.02).max(0.001);
            let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
            let pad = 3.0;
            let pts: Vec<Point<Pixels>> = values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let x = i as f32 / (values.len() - 1) as f32 * w;
                    let y = pad + (1.0 - ((v - min) / span) as f32) * (h - pad * 2.0);
                    point(bounds.origin.x + px(x), bounds.origin.y + px(y))
                })
                .collect();
            // smooth with midpoints as curve ends and the samples as control points
            let curve = |b: &mut PathBuilder| {
                for i in 1..pts.len() {
                    let mid = point((pts[i - 1].x + pts[i].x) / 2.0, (pts[i - 1].y + pts[i].y) / 2.0);
                    b.curve_to(mid, pts[i - 1]);
                }
                b.line_to(*pts.last().unwrap());
            };
            let bottom = bounds.origin.y + bounds.size.height;
            let mut fill_b = PathBuilder::fill();
            fill_b.move_to(point(pts[0].x, bottom));
            fill_b.line_to(pts[0]);
            curve(&mut fill_b);
            fill_b.line_to(point(pts.last().unwrap().x, bottom));
            fill_b.close();
            if let Ok(p) = fill_b.build() {
                window.paint_path(p, linear_gradient(180., linear_color_stop(color.opacity(0.32), 0.), linear_color_stop(color.opacity(0.0), 1.)));
            }
            let mut line = PathBuilder::stroke(px(1.75));
            line.move_to(pts[0]);
            curve(&mut line);
            if let Ok(p) = line.build() {
                window.paint_path(p, color);
            }
            if let Some(p) = dot(*pts.last().unwrap(), 3.0) {
                window.paint_path(p, color);
            }
        },
    )
    .w_full()
    .h(px(height))
}

/// A ring split into coloured parts (fractions of the whole), with a hairline gap between them.
pub fn donut(parts: Vec<(f32, Hsla)>, track: Hsla, size: f32, thickness: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let c = bounds.center();
            let r = size / 2.0 - thickness / 2.0;
            if let Some(p) = arc(c, r, 0.0, 1.0, thickness) {
                window.paint_path(p, track);
            }
            let gap = if parts.len() > 1 { 0.004 } else { 0.0 };
            let mut at = 0.0f32;
            for (f, color) in &parts {
                let to = (at + f).min(1.0);
                if to - at > gap * 2.0 + 0.001
                    && let Some(p) = arc(c, r, at + gap, to - gap, thickness)
                {
                    window.paint_path(p, *color);
                }
                at = to;
            }
        },
    )
    .size(px(size))
    .flex_shrink_0()
}
