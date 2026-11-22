//! The window drawn: the input's panel and the chat's, the chips, the send button.

use super::*;

/// A capture's chip: just its image at its own shape (clicked, `open` shows it enlarged), with
/// an X over its top-right corner on hover (`remove`) that takes no room; `compact` (many of
/// them), smaller.
pub fn chip<S: 'static>(shot: &Attached, compact: bool, remove: impl Fn(&mut S) + 'static, open: impl Fn(&mut S) + 'static) -> El {
    let id = shot.id;
    let preview = shot
        .preview
        .fitted(if compact { THUMB_COMPACT } else { THUMB }, 6.0)
        .key(format!("shot-preview-{id}"))
        .border(1.0, Color::WHITE.alpha(0.16))
        .cursor(Cursor::Pointer)
        .label("Show larger")
        .on_press(open);
    let close = el()
        .key(format!("shot-x-{id}"))
        .size(18.0, 18.0)
        .radius(9.0)
        .center()
        .fill(Color::BLACK.alpha(0.7))
        .border(1.0, Color::WHITE.alpha(0.2))
        .hover(Color::BLACK.alpha(0.9))
        .cursor(Cursor::Pointer)
        .group_opacity(0.0, 1.0)
        .label("Remove image")
        .child(icon(&glyphs::CLOSE, 9.0).color(Color::WHITE))
        .on_press(remove)
        .pin(None, Some(3.0), Some(3.0), None);
    el().key(format!("shot-{id}"))
        .hover_group()
        .transition(Transition::scale(0.9).combined(Transition::opacity()))
        .child(preview)
        .child(close)
}

fn send_button(ready: bool, t: &tok::Theme) -> El {
    let button = el()
        .key("send")
        .size(28.0, 28.0)
        .radius(8.0)
        .continuous_corners()
        .center()
        .label("Send")
        .shortcut_hint("↵");
    if ready {
        button
            .fill(ACCENT)
            .hover(Color::WHITE.alpha(0.10))
            .press(Color::BLACK.alpha(0.12))
            .press_scale(0.94)
            .cursor(Cursor::Pointer)
            .child(icon(&icons::ARROW_UP, 16.0).color(Color::WHITE))
            .on_press(|s: &mut QuickInput| s.submit())
    } else {
        button.fill(Color::WHITE.alpha(0.08)).disabled(true).child(icon(&icons::ARROW_UP, 16.0).color(t.text_secondary))
    }
}

/// The input's and the chat's panel: the system material under the frost, its border, shadows
/// and corners, holding `layers` pinned to its top left (each at its own size, so a panel that
/// reshapes reveals them rather than reflowing them). Keyed `key`; takes clicks if `hit`.
pub fn surface(key: &str, hit: bool, layers: Vec<El>) -> El {
    let mut frost = el()
        .key("frost")
        .w_full()
        .h_full()
        .radius(RADIUS)
        .continuous_corners()
        .clip_rounded(RADIUS)
        .fill(FROST)
        .border_gradient(1.0, Color::WHITE.alpha(0.14), Color::WHITE.alpha(0.05));
    for layer in layers {
        frost = frost.child(layer);
    }
    let panel = el()
        .key(key)
        .radius(RADIUS)
        .continuous_corners()
        .box_shadow(Shadow::core_animation(0.0, 0.5, 1.5, Color::BLACK.alpha(0.3)))
        .box_shadow(Shadow::core_animation(0.0, 10.0, 22.0, Color::BLACK.alpha(0.4)))
        .system_material(SystemMaterial::Popover)
        .child(frost);
    if hit { panel.hit() } else { panel }
}

/// The window covers the display: the input's panel at its rect, and the chat's at the right
/// edge (the input's panel becomes it on send).
pub fn view(s: &QuickInput, cx: &mut Cx<QuickInput>) -> El {
    crate::settings::apply_motion(cx);
    let t = tok::theme(true);
    let Pose { dy, alpha, scale } = s.current_pose();
    let motion = Anim::from(GLIDE);
    let [.., w, h] = s.panel_rect();
    let [x, bottom] = s.pins();
    // Just sent: the row is in the chat's panel, fading; the input's new panel waits empty, out
    // of view, landing at once (its element was the chat's, maybe mid-slide).
    let layers = if s.morphing { Vec::new() } else { vec![input_row(s, &t)] };
    let mut input = surface(s.input_key(), s.open, layers)
        .size(w, h)
        // By its bottom edge: placed for another screen, or none yet at launch, closed it still
        // waits below the window's edge, never on screen.
        .pin(Some(x), None, None, Some(bottom))
        .offset(0.0, dy)
        .offset_motion(motion)
        .opacity(alpha)
        .opacity_motion(motion)
        .scale(scale)
        .scale_motion(if s.pressed { Anim::new(Curve::ease_in_out(PRESS.as_secs_f32())) } else { motion })
        .motion(GLIDE);
    if s.morphing {
        input = input.instant_changes();
    }
    let chat = chat::panel(&s.chat, s.chat_key(), |s: &mut QuickInput| &mut s.chat, s.morphing.then(|| input_row(s, &t)));
    let root = el().w_full().h_full().child(chat).child(input);
    match s.preview_image() {
        Some(image) => root.child(chat::lightbox(image, s.chat.screen(), |s: &mut QuickInput| s.close_preview())),
        None => root,
    }
}

/// The chips in a strip above the field row, then the row (the sparkle, the field, the send
/// button), at the panel's size, pinned to its top left.
fn input_row(s: &QuickInput, t: &tok::Theme) -> El {
    let field = text_input(&s.query, TextStyle::new(15.0, 400.0).lh(22.0))
        .key("query")
        .label("Ask anything")
        .single_line()
        .grow(1.0)
        .min_w(FIELD_MIN_W)
        .color(t.text)
        .caret(t.text, ACCENT.alpha(0.35))
        .placeholder(hint(s.shots.len(), s.capture_chord.as_deref()), t.text_secondary.alpha(0.8))
        .animate_layout()
        .on_edit(|s: &mut QuickInput| &mut s.query)
        .on_input_key(|s: &mut QuickInput, ev: &KeyEvent, _: &mut Tx| s.field_key(ev))
        .on_input_paste(|s: &mut QuickInput, content: &rune::ClipboardContent, _: &mut Tx| s.paste(content))
        .on_submit(|s: &mut QuickInput, _: &mut Tx| s.submit());
    let compact = compact(s.shots.len(), INPUT_FULL_CHIPS);
    let mut chips = row().key("chips").w_full().h(strip_h(s.shots.len())).px(ROW_PAD).pt(STRIP_TOP).gap(8.0).items_end();
    for shot in &s.shots {
        let id = shot.id;
        let (remove, open) = (move |s: &mut QuickInput| _ = s.remove(id), move |s: &mut QuickInput| s.open_preview(id));
        chips = chips.child(chip(shot, compact, remove, open));
    }
    let field_row = row()
        .key("field-row")
        .role("textbox")
        .w_full()
        .h(PANEL_H)
        .px(ROW_PAD)
        .gap(8.0)
        .center_y()
        .child(icon(&glyphs::SPARKLE, 18.0).color(ACCENT).mr(2.0))
        .child(field)
        .child(send_button(s.send_ready(), t).ml(2.0));
    let [_, _, w, h] = s.panel_rect();
    let mut layer = col().key("input-layer").size(w, h).pin(Some(0.0), Some(0.0), None, None);
    if !s.shots.is_empty() {
        layer = layer.child(chips);
    }
    layer.child(field_row)
}

pub fn on_key(s: &mut QuickInput, event: &KeyEvent, _: &mut Tx) -> bool {
    if s.open { s.field_key(event) } else { chat::on_key(&mut s.chat, event) }
}

/// Clicking elsewhere closes it, unless a capture took the keys (the host decides).
pub fn on_focus(s: &mut QuickInput, focused: bool, _: &mut Tx) {
    if !focused && s.is_open() && let Some(host) = &s.host {
        host.update(|host, _| host.quick_input_blurred());
    }
}
