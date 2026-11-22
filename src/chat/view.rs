//! The panel drawn: its header and buttons, the thread of messages and the composer under it.

use super::*;

const HEADER_H: f32 = 44.0;
/// A message's widest, and its images' largest.
const BUBBLE_MAX_W: f32 = CHAT_W * 0.82;
const THUMB_MAX: (f32, f32) = (240.0, 160.0);

fn close_button<S: 'static>(t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
    el().key("close")
        .size(26.0, 26.0)
        .radius(7.0)
        .continuous_corners()
        .center()
        .hover(Color::WHITE.alpha(0.10))
        .press(Color::WHITE.alpha(0.06))
        .cursor(Cursor::Pointer)
        .label("Close")
        .shortcut_hint("Esc")
        .child(icon(&glyphs::CLOSE, 12.0).color(t.text_secondary))
        .on_press(move |s: &mut S| lens(s).dismiss())
}

/// The model writing the replies, beside Send: it lists the models to pick from.
fn model_picker<S: 'static>(s: &Chat, t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
    let label = s.model_label();
    row()
        .key("model-picker")
        .h(28.0)
        .px(8.0)
        .gap(4.0)
        .radius(8.0)
        .continuous_corners()
        .center_y()
        .hover(Color::WHITE.alpha(0.08))
        .press(Color::WHITE.alpha(0.05))
        .cursor(Cursor::Pointer)
        .role("button")
        .label(format!("Model: {label}"))
        .expanded(s.models_open)
        .child(text(label).font(TextStyle::new(12.0, 500.0)).color(t.text_secondary).single_line())
        .child(icon(if s.models_open { &glyphs::CHEVRON_DOWN } else { &glyphs::CHEVRON_UP }, 10.0).color(t.text_secondary))
        .on_press(move |s: &mut S| lens(s).toggle_models())
}

/// The models, the one writing marked: a pick closes the list.
fn model_list<S: 'static>(s: &Chat, t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
    let now = s.model();
    let mut list = row().key("models").w_full().gap(6.0).center_y();
    for (id, label) in model::MODELS {
        let on = id == now;
        let b = el()
            .key(format!("model-{id}"))
            .h(26.0)
            .px(10.0)
            .radius(7.0)
            .continuous_corners()
            .center()
            .fill(if on { ACCENT.alpha(0.25) } else { Color::WHITE.alpha(0.06) })
            .border(1.0, if on { ACCENT.alpha(0.6) } else { Color::WHITE.alpha(0.08) })
            .hover(Color::WHITE.alpha(0.10))
            .cursor(Cursor::Pointer)
            .role("radio")
            .label(label)
            .checked(on)
            .child(text(label).font(TextStyle::new(12.0, 500.0)).color(t.text))
            .on_press(move |s: &mut S| lens(s).pick_model(id));
        list = list.child(b);
    }
    list
}

fn send_button<S: 'static>(ready: bool, streaming: bool, t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
    if streaming && !ready {
        return reply_view::stop_button(lens);
    }
    let button = el().key("send").size(28.0, 28.0).radius(8.0).continuous_corners().center().label("Send").shortcut_hint("↵");
    if ready {
        button
            .fill(ACCENT)
            .hover(Color::WHITE.alpha(0.10))
            .press(Color::BLACK.alpha(0.12))
            .press_scale(0.94)
            .cursor(Cursor::Pointer)
            .child(icon(&icons::ARROW_UP, 16.0).color(Color::WHITE))
            .on_press_tx(move |s: &mut S, tx: &mut Tx| send(lens(s), tx))
    } else {
        button.fill(Color::WHITE.alpha(0.08)).disabled(true).child(icon(&icons::ARROW_UP, 16.0).color(t.text_secondary))
    }
}

/// Enter or the send button: the draft and its captures join the conversation; the captures
/// are decoded off the main thread and show when ready.
/// Over the thread's bottom while the user is scrolled back: back to the latest.
fn to_latest<S: 'static>(t: &tok::Theme) -> El {
    let button = el()
        .key("to-latest")
        .size(32.0, 32.0)
        .radius(16.0)
        .center()
        .fill(Color { r: 0.16, g: 0.16, b: 0.16, a: 0.96 })
        .border(1.0, Color::WHITE.alpha(0.14))
        .shadow(4.0, 12.0, Color::BLACK.alpha(0.35))
        .hover(Color::WHITE.alpha(0.08))
        .press(Color::BLACK.alpha(0.12))
        .press_scale(0.94)
        .cursor(Cursor::Pointer)
        .label("Jump to latest")
        .child(icon(&glyphs::CHEVRON_DOWN, 14.0).color(t.text))
        .on_press_tx(move |_: &mut S, tx: &mut Tx| tx.scroll_to_bottom("messages"));
    row()
        .key("to-latest-row")
        .pin(Some(0.0), None, Some(0.0), Some(10.0))
        .justify_center()
        .transition(Transition::opacity().combined(Transition::scale(0.85)))
        .child(button)
}

fn send(s: &mut Chat, tx: &mut Tx) {
    let Some((id, pngs)) = s.send() else { return };
    s.ask(id);
    tx.scroll_to_bottom("messages");
    if !pngs.is_empty()
        && let Some(host) = &s.host
    {
        host.update(move |host, _| host.decode_into_chat(id, pngs));
    }
}

/// A sent message, on the right: its captures, then its text in a bubble.
fn message(i: usize, m: &Message, t: &tok::Theme) -> El {
    let mut out = col().key(format!("msg-{i}")).w_full().items_end().gap(6.0).arrives_whole().transition(
        Transition::offset(0.0, 8.0).combined(Transition::opacity()),
    );
    for (j, image) in m.images.iter().enumerate() {
        let (w, h) = thumb_size(image.width, image.height, THUMB_MAX);
        out = out.child(
            el().key(format!("msg-{i}-img-{j}"))
                .size(w, h)
                .radius(10.0)
                .continuous_corners()
                .clip_rounded(10.0)
                .border(1.0, Color::WHITE.alpha(0.10))
                .pixels(&image.pixels),
        );
    }
    if !m.text.is_empty() {
        out = out.child(
            el().key(format!("msg-{i}-text"))
                .max_w(BUBBLE_MAX_W)
                .px(12.0)
                .py(8.0)
                .radius(14.0)
                .continuous_corners()
                .fill(Color::WHITE.alpha(0.08))
                .child(text(m.text.clone()).font(TextStyle::new(13.5, 400.0).lh(20.0)).color(t.text).wrap()),
        );
    }
    out
}

/// The panel, keyed `key`, in a window whose state `S` holds the chat at `lens`. The input's row
/// (`input_row`), while the input's panel is turning into this one, fades out over it.
pub fn panel<S: 'static>(s: &Chat, key: &str, lens: fn(&mut S) -> &mut Chat, input_row: Option<El>) -> El {
    let t = tok::theme(true);
    let header = header(14.0, &t)
        .child(header_button("pop-out", &icons::POP_OUT, "Open in a window", &t, move |s: &mut S| lens(s).pop_out()))
        .child(close_button(&t, lens));
    let [.., w, h] = side_rect(s.screen, s.insets);
    let [top, right] = side_pins(s.insets);
    // At its landed size from the start, pinned to the panel's top left: the panel reveals it as
    // it grows. Hidden while the input's panel reshapes, then faded in.
    let content = body(s, header, lens)
        .key("chat-layer")
        .size(w, h)
        .pin(Some(0.0), Some(0.0), None, None)
        .opacity(if s.landed { 1.0 } else { 0.0 })
        .opacity_motion(Anim::new(Curve::ease_in_out(CONTENT_FADE)));
    let layers = match input_row {
        Some(row) => vec![content, row.opacity(0.0).opacity_motion(Anim::new(Curve::ease_in_out(ROW_FADE)))],
        None => vec![content],
    };
    quick_input::surface(key, s.open, layers)
        .size(w, h)
        // By its right edge: told the wrong screen (none yet at launch, or another display's),
        // closed it still waits past the window's edge, never on screen.
        .pin(None, Some(top), Some(right), None)
        .offset(offset(s.open, s.insets), 0.0)
        .offset_motion(Anim::from(GLIDE))
        // The input's panel arrives pressed in (sending) and springs back to full size.
        .scale(1.0)
        .scale_motion(Anim::from(GLIDE))
        .motion(GLIDE)
}

/// The header's row: the sparkle and "Chat", `lead` points in; the caller adds its buttons.
pub fn header(lead: f32, t: &tok::Theme) -> El {
    row()
        .key("header")
        .w_full()
        .h(HEADER_H)
        .pl(lead)
        .pr(9.0)
        .gap(8.0)
        .center_y()
        .child(icon(&glyphs::SPARKLE, 16.0).color(ACCENT))
        .child(text("Chat").font(TextStyle::new(13.5, 600.0)).color(t.text).grow(1.0))
}

pub const HEADER_HEIGHT: f32 = HEADER_H;

/// A header button: a glyph, its `label` spoken and shown on hover.
fn header_button<S: 'static>(key: &str, glyph: &'static rune::Icon, label: &str, t: &tok::Theme, f: impl Fn(&mut S) + 'static) -> El {
    el().key(key.to_owned())
        .size(26.0, 26.0)
        .radius(7.0)
        .continuous_corners()
        .center()
        .hover(Color::WHITE.alpha(0.10))
        .press(Color::WHITE.alpha(0.06))
        .cursor(Cursor::Pointer)
        .label(label.to_owned())
        .child(icon(glyph, 14.0).color(t.text_secondary))
        .on_press(f)
}

/// Its window's dock button: back into the side panel.
pub fn dock_button<S: 'static>(t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
    header_button("dock-side", &icons::DOCK_SIDE, "Back to the side panel", t, move |s: &mut S| lens(s).dock_back())
}

/// The conversation under `header`: the thread, scrolling, then the composer.
pub fn body<S: 'static>(s: &Chat, header: El, lens: fn(&mut S) -> &mut Chat) -> El {
    let t = tok::theme(true);
    let divider = || el().w_full().h(1.0).fill(Color::WHITE.alpha(0.06));
    let mut thread = col().key("thread").w_full().gap(14.0).px(14.0).py(14.0).growth(GrowthSpec::default());
    let goes_on = s.continuable();
    for (i, m) in s.messages.iter().enumerate() {
        thread = thread.child(message(i, m, &t));
        if let Some(r) = s.reply_to(m.id) {
            thread = thread.child(reply_view::view(r, goes_on == Some(r.token), lens));
        }
    }
    let messages = col()
        .key("messages")
        .w_full()
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        // Follows what comes while at the bottom; scrolled back, it stays put.
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(16.0)
        .on_at_bottom(move |s: &mut S, at: bool| lens(s).away = !at)
        .child(thread);
    let mut messages = col().key("thread-frame").w_full().grow(1.0).shrink().min_h(0.0).child(messages);
    if s.away {
        messages = messages.child(to_latest::<S>(&t));
    }
    let field = text_input(&s.draft, TextStyle::new(14.0, 400.0).lh(20.0))
        .key("draft")
        .label("Message")
        .w_full()
        .color(t.text)
        .caret(t.text, ACCENT.alpha(0.35))
        .placeholder("Message…", t.text_secondary.alpha(0.8))
        .on_edit(move |s: &mut S| &mut lens(s).draft)
        .on_input_key(move |s: &mut S, ev: &KeyEvent, _: &mut Tx| lens(s).field_key(ev))
        .on_input_paste(move |s: &mut S, content: &rune::ClipboardContent, _: &mut Tx| lens(s).paste(content))
        .on_submit(move |s: &mut S, tx: &mut Tx| send(lens(s), tx));
    let typing = row()
        .key("typing")
        .w_full()
        .items_end()
        .gap(8.0)
        .child(col().grow(1.0).min_w(0.0).py(4.0).child(auto_grow(field, 1, 6)))
        .child(model_picker(s, &t, lens))
        .child(send_button(s.send_ready(), s.streaming(), &t, lens));
    let mut composer = col()
        .key("composer")
        .w_full()
        .gap(6.0)
        .pl(12.0)
        .pr(8.0)
        .py(8.0)
        .radius(12.0)
        .continuous_corners()
        .fill(Color::WHITE.alpha(0.05))
        .border(1.0, Color::WHITE.alpha(0.08));
    // Captures pasted in, as the input's chips.
    if !s.shots.is_empty() {
        let compact = quick_input::compact(s.shots.len(), quick_input::CHAT_FULL_CHIPS);
        let mut chips = row().key("chips").gap(6.0).pt(2.0).center_y();
        for shot in &s.shots {
            let id = shot.id;
            let (remove, open) = (move |s: &mut S| _ = lens(s).remove(id), move |s: &mut S| lens(s).open_preview(id));
            chips = chips.child(quick_input::chip(shot, compact, remove, open));
        }
        composer = composer.child(chips);
    }
    if s.models_open {
        composer = composer.child(model_list(s, &t, lens));
    }
    let composer = composer.child(typing);
    col().child(header).child(divider()).child(messages).child(el().w_full().padding(10.0).child(composer))
}

/// Keys in the window with the chat's field focused and nothing else taking them.
pub fn on_key(s: &mut Chat, event: &KeyEvent) -> bool {
    s.open && s.field_key(event)
}
