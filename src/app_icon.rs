//! The app's icon: Ailfred's mark (a top hat over a bow tie). White on a dark rounded tile for
//! the Dock, the app switcher and the Finder (the bundle's `.icns` comes from `--app-icon`); alone,
//! as a template, in the menu bar.

use rune::Icon;

/// The mark's shapes on a 256-unit grid: the hat's crown and brim, and the bow tie's two wings
/// around its knot.
macro_rules! mark {
    () => {
        concat!(
            r#"<path d="M 49 112 L 49 84 C 49 36, 84 12, 128 12 C 172 12, 207 36, 207 84 L 207 112 C 156 123, 100 123, 49 112 Z"/>"#,
            r#"<path d="M 27 116 C 18 115, 13 123, 17 131 C 21 139, 64 152, 128 152 C 192 152, 235 139, 239 131 C 243 123, 238 115, 229 116 C 221 117, 214 121, 204 124 C 154 134, 102 134, 52 124 C 42 121, 35 117, 27 116 Z"/>"#,
            r#"<path d="M 110 198 L 75 182 C 69 180, 64 184, 64 191 L 64 227 C 64 234, 69 238, 75 236 L 110 220 Z"/>"#,
            r#"<rect x="115" y="195" width="26" height="28" rx="6"/>"#,
            r#"<path d="M 146 198 L 181 182 C 187 180, 192 184, 192 191 L 192 227 C 192 234, 187 238, 181 236 L 146 220 Z"/>"#,
        )
    };
}

/// The mark alone, edge to edge.
#[cfg(test)]
pub static MARK: Icon = Icon {
    name: "ailfred-mark",
    view: 256.0,
    body: concat!(r#"<g fill="currentColor">"#, mark!(), "</g>"),
};

/// The mark in the menu bar: at 80%, centred, so it keeps a margin like the system's icons
/// (about 16 of the bar's 22 pt).
pub static MENU_BAR_MARK: Icon = Icon {
    name: "ailfred-menu-bar-mark",
    view: 256.0,
    body: concat!(r#"<g transform="translate(25.6 25.6) scale(0.8)" fill="currentColor">"#, mark!(), "</g>"),
};

/// The tile, on macOS's icon grid: 824 of 1024 units, inset 100, corners of 185.
static TILE: Icon = Icon {
    name: "ailfred-tile",
    view: 1024.0,
    body: r#"<rect x="100" y="100" width="824" height="824" rx="185" fill="currentColor"/>"#,
};

/// The mark on the tile's grid: twice its size (about 56% of the tile), centred.
static MARK_ON_TILE: Icon = Icon {
    name: "ailfred-mark-on-tile",
    view: 1024.0,
    body: concat!(r#"<g transform="translate(256 262) scale(2)" fill="currentColor">"#, mark!(), "</g>"),
};

/// The tile's colour: the chat panel's, #1E1E1E (sRGB bytes).
const TILE_RGB: [u8; 3] = [0x1e, 0x1e, 0x1e];

/// The icon `px` pixels square, RGBA (sRGB, straight alpha): the white mark on the dark tile.
pub fn rgba(px: u32) -> Vec<u8> {
    let tile = rune::text::icon_coverage(&TILE, px);
    let mark = rune::text::icon_coverage(&MARK_ON_TILE, px);
    // The mark lies inside the tile: white over it by the mark's coverage, the tile's alpha.
    let over = |bg: u8, m: u8| (u32::from(bg) + (255 - u32::from(bg)) * u32::from(m) / 255) as u8;
    tile.iter()
        .zip(&mark)
        .flat_map(|(&t, &m)| {
            let [r, g, b] = TILE_RGB;
            [over(r, m), over(g, m), over(b, m), t.max(m)]
        })
        .collect()
}

/// [`rgba`] as a PNG.
pub fn png(px: u32) -> Vec<u8> {
    crate::export::encode_png(px, px, 1.0, &rgba(px)).expect("an in-memory PNG encodes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rgba: &[u8], px: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * px + x) * 4) as usize;
        rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn the_app_icon_is_the_white_mark_on_a_dark_rounded_tile() {
        let rgba = rgba(256);
        assert_eq!(rgba.len(), 256 * 256 * 4);
        assert_eq!(at(&rgba, 256, 2, 2)[3], 0, "outside the tile: transparent");
        assert_eq!(at(&rgba, 256, 28, 28)[3], 0, "the tile's rounded corner");
        assert_eq!(at(&rgba, 256, 40, 128), [0x1e, 0x1e, 0x1e, 255], "the tile");
        assert_eq!(at(&rgba, 256, 128, 95), [255, 255, 255, 255], "the hat's crown");
        assert_eq!(at(&rgba, 256, 128, 170), [255, 255, 255, 255], "the bow tie's knot");
        assert_eq!(at(&rgba, 256, 128, 149), [0x1e, 0x1e, 0x1e, 255], "between hat and bow tie");
    }

    #[test]
    fn its_png_decodes_to_the_same_pixels() {
        let bytes = png(64);
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().expect("a PNG");
        let mut out = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut out).unwrap();
        assert_eq!((info.width, info.height, info.color_type), (64, 64, png::ColorType::Rgba));
        assert_eq!(&out[..info.buffer_size()], rgba(64).as_slice());
    }
}
