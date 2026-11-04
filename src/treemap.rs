//! Squarified treemap layout (Bruls, Huizing, van Wijk) — pure geometry, no UI.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn inset(&self, l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect { x: self.x + l, y: self.y + t, w: (self.w - l - r).max(0.0), h: (self.h - t - b).max(0.0) }
    }

}

/// Lays `values` (sorted largest first, all > 0) out inside `rect`.
/// Returns one rectangle per value, in the same order.
pub fn squarify(values: &[f64], rect: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::default(); values.len()];
    let total: f64 = values.iter().sum();
    if total <= 0.0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return out;
    }
    let (mut x, mut y, mut w, mut h) = (rect.x as f64, rect.y as f64, rect.w as f64, rect.h as f64);
    let scale = w * h / total;
    let areas: Vec<f64> = values.iter().map(|v| v * scale).collect();

    let mut i = 0;
    while i < areas.len() {
        let side = w.min(h);
        if side <= 0.0 {
            break;
        }
        // grow the row while the worst aspect ratio improves
        let mut j = i;
        let mut sum = 0.0;
        let mut best = f64::INFINITY;
        while j < areas.len() {
            let s = sum + areas[j];
            let worst = worst_ratio(&areas[i..=j], s, side);
            if j > i && worst > best {
                break;
            }
            best = worst;
            sum = s;
            j += 1;
        }
        let thick = sum / side;
        let mut off = 0.0;
        if w >= h {
            for k in i..j {
                let len = areas[k] / thick;
                out[k] = Rect::new(x as f32, (y + off) as f32, thick as f32, len as f32);
                off += len;
            }
            x += thick;
            w -= thick;
        } else {
            for k in i..j {
                let len = areas[k] / thick;
                out[k] = Rect::new((x + off) as f32, y as f32, len as f32, thick as f32);
                off += len;
            }
            y += thick;
            h -= thick;
        }
        i = j;
    }
    out
}

fn worst_ratio(row: &[f64], sum: f64, side: f64) -> f64 {
    let (mut max, mut min) = (f64::MIN, f64::MAX);
    for &a in row {
        max = max.max(a);
        min = min.min(a);
    }
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_the_rect() {
        let r = Rect::new(10.0, 20.0, 600.0, 400.0);
        let vals = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rects = squarify(&vals, r);
        let area: f32 = rects.iter().map(|q| q.w * q.h).sum();
        assert!((area - 240_000.0).abs() < 1.0, "area {area}");
        let total: f64 = vals.iter().sum();
        for (q, v) in rects.iter().zip(vals) {
            let expected = (v / total * 240_000.0) as f32;
            assert!((q.w * q.h - expected).abs() < 1.0);
            assert!(q.x >= 10.0 - 0.01 && q.y >= 20.0 - 0.01);
            assert!(q.x + q.w <= 610.01 && q.y + q.h <= 420.01);
        }
    }

    #[test]
    fn empty_input() {
        assert!(squarify(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
    }
}
