//! Annotations on a selection: the model, its geometry, and pixelation. Coordinates are points
//! relative to the selection's top-left; the export multiplies them by the display's scale.

/// Pixelation block, in points.
pub const BLOCK: f32 = 10.0;
/// A mark smaller than this (points, both sides for boxes, length for arrows) is a slip.
pub const MIN_MARK: f32 = 4.0;

/// A mark's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Red,
    Yellow,
    Green,
    Blue,
    White,
    Black,
}

impl Ink {
    pub const ALL: [Ink; 6] = [Ink::Red, Ink::Yellow, Ink::Green, Ink::Blue, Ink::White, Ink::Black];
    /// sRGB, 0..1.
    pub fn rgb(self) -> [f32; 3] {
        match self {
            Ink::Red => [0.96, 0.18, 0.2],
            Ink::Yellow => [1.0, 0.8, 0.0],
            Ink::Green => [0.2, 0.78, 0.35],
            Ink::Blue => [0.0, 0.48, 1.0],
            Ink::White => [1.0, 1.0, 1.0],
            Ink::Black => [0.0, 0.0, 0.0],
        }
    }
}

/// A mark's size: stroke width and text size together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    S,
    M,
    L,
}

impl Size {
    /// Stroke width of boxes and arrows, in points.
    pub fn stroke(self) -> f32 {
        match self {
            Size::S => 2.0,
            Size::M => 4.0,
            Size::L => 7.0,
        }
    }
    /// Text size, in points.
    pub fn text(self) -> f32 {
        match self {
            Size::S => 16.0,
            Size::M => 22.0,
            Size::L => 30.0,
        }
    }
    pub fn next(self) -> Size {
        match self {
            Size::S => Size::M,
            Size::M => Size::L,
            Size::L => Size::S,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Size::S => "S",
            Size::M => "M",
            Size::L => "L",
        }
    }
}

/// How a mark is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub ink: Ink,
    pub size: Size,
}

impl Default for Style {
    /// Red, medium: the look before styles existed.
    fn default() -> Style {
        Style { ink: Ink::Red, size: Size::M }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Arrow,
    Rect,
    Pixelate,
    Text,
}

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::Arrow, Tool::Rect, Tool::Pixelate, Tool::Text];
    pub fn label(self) -> &'static str {
        match self {
            Tool::Arrow => "Arrow",
            Tool::Rect => "Box",
            Tool::Pixelate => "Pixelate",
            Tool::Text => "Text",
        }
    }
    /// The key that picks it.
    pub fn key(self) -> &'static str {
        match self {
            Tool::Arrow => "a",
            Tool::Rect => "r",
            Tool::Pixelate => "p",
            Tool::Text => "t",
        }
    }
    pub fn from_key(key: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.key().eq_ignore_ascii_case(key))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Arrow { from: [f32; 2], to: [f32; 2], style: Style },
    /// x, y, width, height.
    Rect([f32; 4], Style),
    Pixelate([f32; 4]),
    /// Top-left of the text line.
    Text { at: [f32; 2], text: String, style: Style },
}

impl Mark {
    /// A mark dragged from `a` to `b` with a drawing tool (not Text).
    pub fn dragged(tool: Tool, a: [f32; 2], b: [f32; 2], style: Style) -> Option<Mark> {
        match tool {
            Tool::Arrow => Some(Mark::Arrow { from: a, to: b, style }),
            Tool::Rect => Some(Mark::Rect(rect(a, b), style)),
            Tool::Pixelate => Some(Mark::Pixelate(rect(a, b))),
            Tool::Text => None,
        }
    }

    /// Big enough to keep (a click or a twitch isn't a mark).
    pub fn is_meaningful(&self) -> bool {
        match self {
            Mark::Arrow { from, to, .. } => (to[0] - from[0]).hypot(to[1] - from[1]) >= MIN_MARK,
            Mark::Rect(r, _) | Mark::Pixelate(r) => r[2] >= MIN_MARK && r[3] >= MIN_MARK,
            Mark::Text { text, .. } => !text.trim().is_empty(),
        }
    }
}

/// The rectangle spanned by two corners.
pub fn rect(a: [f32; 2], b: [f32; 2]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), (b[0] - a[0]).abs(), (b[1] - a[1]).abs()]
}

/// Clamps a point into a `w` × `h` area.
pub fn clamp(p: [f32; 2], w: f32, h: f32) -> [f32; 2] {
    [p[0].clamp(0.0, w), p[1].clamp(0.0, h)]
}

/// An arrow's shaft end and head triangle: the shaft stops at the head's base so the round cap
/// doesn't poke through the tip. Head size grows with the stroke, capped at a third of the length.
pub fn arrow_geometry(from: [f32; 2], to: [f32; 2], stroke: f32) -> ([f32; 2], [[f32; 2]; 3]) {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = dx.hypot(dy).max(f32::EPSILON);
    let (ux, uy) = (dx / len, dy / len);
    let head = (stroke * 4.5).min(len / 3.0).max(stroke * 1.5);
    let half = head * 0.6;
    let base = [to[0] - ux * head, to[1] - uy * head];
    let left = [base[0] - uy * half, base[1] + ux * half];
    let right = [base[0] + uy * half, base[1] - ux * half];
    (base, [to, left, right])
}

/// Pixelates `rect` (pixels, clamped) of a `width`-wide RGBA buffer in place: each `block`-pixel
/// square becomes its average colour.
pub fn pixelate(rgba: &mut [u8], width: usize, height: usize, rect: [usize; 4], block: usize) {
    let block = block.max(1);
    let [rx, ry, rw, rh] = rect;
    let (x1, y1) = ((rx + rw).min(width), (ry + rh).min(height));
    let mut by = ry;
    while by < y1 {
        let bh = block.min(y1 - by);
        let mut bx = rx;
        while bx < x1 {
            let bw = block.min(x1 - bx);
            let mut sum = [0u32; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = (y * width + x) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(rgba[i + c]);
                    }
                }
            }
            let n = (bw * bh) as u32;
            let avg = sum.map(|s| (s / n) as u8);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = (y * width + x) * 4;
                    rgba[i..i + 4].copy_from_slice(&avg);
                }
            }
            bx += bw;
        }
        by += bh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_from_any_two_corners() {
        assert_eq!(rect([10.0, 20.0], [4.0, 30.0]), [4.0, 20.0, 6.0, 10.0]);
    }

    #[test]
    fn slips_are_not_marks() {
        assert!(!Mark::dragged(Tool::Rect, [0.0, 0.0], [2.0, 50.0], Style::default()).unwrap().is_meaningful());
        assert!(Mark::dragged(Tool::Arrow, [0.0, 0.0], [30.0, 0.0], Style::default()).unwrap().is_meaningful());
        assert!(Mark::dragged(Tool::Text, [0.0, 0.0], [30.0, 0.0], Style::default()).is_none());
        assert!(!Mark::Text { at: [0.0, 0.0], text: "  ".into(), style: Style::default() }.is_meaningful());
    }

    #[test]
    fn arrow_head_points_at_the_tip() {
        let (base, head) = arrow_geometry([0.0, 0.0], [100.0, 0.0], 4.0);
        assert_eq!(head[0], [100.0, 0.0]);
        assert!(base[0] < 100.0 && base[1].abs() < 1e-4, "shaft stops before the tip");
        assert!((head[1][1] + head[2][1]).abs() < 1e-4, "head symmetric about the shaft");
    }

    #[test]
    fn pixelate_averages_blocks_inside_the_rect_only() {
        // 4x2: left 2x2 block black/white checker, right half untouched red.
        let mut px = vec![0u8; 4 * 2 * 4];
        let set = |px: &mut Vec<u8>, x: usize, y: usize, c: [u8; 4]| px[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4].copy_from_slice(&c);
        set(&mut px, 0, 0, [0, 0, 0, 255]);
        set(&mut px, 1, 0, [255, 255, 255, 255]);
        set(&mut px, 0, 1, [255, 255, 255, 255]);
        set(&mut px, 1, 1, [0, 0, 0, 255]);
        for (x, y) in [(2, 0), (3, 0), (2, 1), (3, 1)] {
            set(&mut px, x, y, [255, 0, 0, 255]);
        }
        pixelate(&mut px, 4, 2, [0, 0, 2, 2], 2);
        assert_eq!(&px[0..4], &[127, 127, 127, 255]);
        assert_eq!(&px[(1 * 4 + 1) * 4..(1 * 4 + 1) * 4 + 4], &[127, 127, 127, 255]);
        assert_eq!(&px[2 * 4..2 * 4 + 4], &[255, 0, 0, 255], "outside the rect untouched");
    }

    #[test]
    fn sizes_cycle_and_default_is_the_old_look() {
        assert_eq!(Size::S.next().next().next(), Size::S);
        let d = Style::default();
        assert_eq!((d.ink, d.size.stroke(), d.size.text()), (Ink::Red, 4.0, 22.0));
    }

    #[test]
    fn tool_keys() {
        assert_eq!(Tool::from_key("R"), Some(Tool::Rect));
        assert_eq!(Tool::from_key("x"), None);
    }
}
