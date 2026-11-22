//! Drawing a [`Reply`] in the chat: one component per kind of part (thinking, markdown with
//! framed code blocks, agent steps, a diff, a question, an error), and the actions under it.

use std::time::Instant;

use rune::tok::{self, Family};
use rune::{Color, Cursor, Cycle, El, Icon, Span, TextStyle, Transition, Tx, col, el, icon, oklch, rich, row, text, text_input};
use rune_kit::{CodeFrame, Markdown, MarkdownStyle, glyphs};

use crate::chat::Chat;
use crate::quick_input::ACCENT;
use crate::reply::{Agent, Diff, Part, Question, Reply, State, Status, Step, Thinking};

const BODY: TextStyle = TextStyle::new(13.5, 400.0).lh(20.0);
const SMALL: TextStyle = TextStyle::new(12.0, 400.0).lh(16.0);
const SMALL_MEDIUM: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0);
const MONO: TextStyle = TextStyle::new(12.0, 400.0).lh(18.0).family(Family::SystemMono);
const MONO_SMALL: TextStyle = TextStyle::new(11.0, 400.0).lh(16.0).family(Family::SystemMono);

/// A card's fill and edge on the frosted panel.
const CARD: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.04 };
const EDGE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 };
const CODE_BG: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.28 };

fn green() -> Color {
    oklch(0.78, 0.15, 150.0)
}

fn red() -> Color {
    oklch(0.70, 0.17, 25.0)
}

/// Something still working: it breathes.
const PULSE: Cycle = Cycle { period: 1.4, phase: 0.0, stops: &[(0.0, 0.35), (0.5, 1.0), (1.0, 0.35)], still: 0.7, fps: 30.0 };

type Lens<S> = fn(&mut S) -> &mut Chat;

/// Reply `r` in a window whose state `S` holds the chat at `lens`.
pub fn view<S: 'static>(r: &Reply, lens: Lens<S>) -> El {
    let t = tok::theme(true);
    let token = r.token;
    let mut out = col().key(format!("reply-{token}")).w_full().gap(10.0).hover_group().transition(Transition::opacity());
    if r.state == State::Queued {
        return out.child(text("Queued").key("queued").font(SMALL_MEDIUM).color(t.text_secondary).label("Queued: it goes once the reply before it ends"));
    }
    let last = r.parts.len().saturating_sub(1);
    for (j, part) in r.parts.iter().enumerate() {
        let live = r.streaming() && j == last;
        let part = match part {
            Part::Thinking(th) => thinking(token, j, th, &t, lens),
            Part::Text(source) => markdown::<S>(source, live, &t),
            Part::Agent(a) => agent(token, j, a, &t, lens),
            Part::Diff(d) => diff(token, j, d, &t, lens),
            Part::Question(q) => question(token, j, q, &t, lens),
            Part::Error(message) => error(token, message, &t, lens),
        };
        out = out.child(part.key(format!("p{j}")).transition(Transition::opacity()));
    }
    if r.parts.is_empty() && r.streaming() {
        out = out.child(waiting(&t));
    }
    if !r.streaming() {
        out = out.child(footer(r, &t, lens));
    }
    out
}

/// Before anything came: three dots in turn.
fn waiting(t: &tok::Theme) -> El {
    const STOPS: &[(f32, f32)] = &[(0.0, 0.25), (0.3, 1.0), (0.6, 0.25), (1.0, 0.25)];
    let mut dots = row().key("waiting").h(20.0).gap(4.0).center_y();
    for k in 0..3 {
        dots = dots.child(el().key(format!("d{k}")).size(6.0, 6.0).radius(3.0).fill(t.text_secondary).cycle(Cycle {
            period: 1.2,
            phase: k as f32 / 6.0,
            stops: STOPS,
            still: 0.6,
            fps: 30.0,
        }));
    }
    dots
}

fn chevron(open: bool, t: &tok::Theme) -> El {
    icon(if open { &glyphs::CHEVRON_DOWN } else { &glyphs::CARET_RIGHT }, 10.0).color(t.text_secondary)
}

/// A small square button with a glyph.
fn icon_button<S: 'static>(key: &str, glyph: &'static Icon, label: &str, t: &tok::Theme, f: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    el().key(key.to_owned())
        .size(24.0, 24.0)
        .radius(6.0)
        .continuous_corners()
        .center()
        .hover(Color::WHITE.alpha(0.10))
        .press(Color::WHITE.alpha(0.06))
        .hover_alone()
        .cursor(Cursor::Pointer)
        .label(label.to_owned())
        .child(icon(glyph, 13.0).color(t.text_secondary))
        .on_press_tx(f)
}

/// A small button with a word.
fn word_button<S: 'static>(key: &str, word: &str, fill: Color, ink: Color, enabled: bool, f: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let b = el().key(key.to_owned()).h(24.0).px(10.0).radius(6.0).continuous_corners().center().fill(fill).child(text(word.to_owned()).font(SMALL_MEDIUM).color(ink));
    if enabled {
        b.hover(Color::WHITE.alpha(0.10)).press(Color::BLACK.alpha(0.12)).hover_alone().cursor(Cursor::Pointer).on_press_tx(f)
    } else {
        b.opacity(0.5).disabled(true)
    }
}

fn toggle<S: 'static>(lens: Lens<S>, token: u64, part: usize, step: Option<usize>) -> impl Fn(&mut S) + 'static {
    move |s: &mut S| {
        if let Some(r) = lens(s).reply_mut(token) {
            r.toggle(part, step);
        }
    }
}

fn thinking<S: 'static>(token: u64, j: usize, th: &Thinking, t: &tok::Theme, lens: Lens<S>) -> El {
    let label = match th.took {
        None => "Thinking…".to_owned(),
        Some(took) => format!("Thought for {}s", took.as_secs().max(1)),
    };
    let mut head = row()
        .key("head")
        .h(22.0)
        .gap(6.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .role("button")
        .label(label.clone())
        .expanded(th.open)
        .child(icon(&glyphs::SPARKLE, 12.0).color(t.text_secondary))
        .child(text(label).font(SMALL_MEDIUM).color(t.text_secondary))
        .child(chevron(th.open, t))
        .on_press(toggle(lens, token, j, None));
    if th.took.is_none() {
        head = head.cycle(PULSE);
    }
    let mut out = col().w_full().gap(6.0).child(head);
    if th.open && !th.text.trim().is_empty() {
        out = out.child(
            row().key("body").w_full().gap(10.0).child(el().w(2.0).h_full().radius(1.0).fill(EDGE)).child(
                col().grow(1.0).min_w(0.0).child(text(th.text.trim().to_owned()).font(SMALL).color(t.text_secondary).wrap()),
            ),
        );
    }
    out
}

fn md_style(t: &tok::Theme) -> MarkdownStyle {
    MarkdownStyle::neutral(true)
        .text(t.text)
        .muted(t.text_secondary)
        .link(ACCENT.mix(Color::WHITE, 0.35))
        .body_font(BODY)
        .code_block(Color::TRANSPARENT, Color::TRANSPARENT)
        .code_block_font(MONO)
        .inline_code_bg(Color::WHITE.alpha(0.08))
        .diff(green().alpha(0.14), red().alpha(0.14))
        .diff_text(green(), red())
        .rule(EDGE)
        .quote_rule(Color::WHITE.alpha(0.2))
}

/// Markdown, its words fading in while it streams (`live`); code blocks framed with their
/// language and a copy button.
fn markdown<S: 'static>(source: &str, live: bool, t: &tok::Theme) -> El {
    let t2 = *t;
    col().w_full().child(
        Markdown::new(source.to_owned())
            .streaming(live)
            .reveal(if live { 0.18 } else { 0.0 })
            .selectable(true)
            .code_frame(move |code, frame| code_frame::<S>(code, frame, &t2))
            .render(&md_style(t)),
    )
}

fn code_frame<S: 'static>(code: El, frame: &CodeFrame<'_>, t: &tok::Theme) -> El {
    let source = frame.source.to_owned();
    let language = frame.language.unwrap_or("text").to_owned();
    let head = row()
        .key("code-head")
        .w_full()
        .h(28.0)
        .pl(10.0)
        .pr(3.0)
        .center_y()
        .fill(Color::WHITE.alpha(0.04))
        .child(text(language).font(SMALL).color(t.text_secondary).grow(1.0))
        .child(icon_button::<S>("copy", &glyphs::COPY, "Copy code", t, move |_, tx| tx.copy(source.clone())));
    col().w_full().radius(10.0).continuous_corners().clip_rounded(10.0).fill(CODE_BG).border(1.0, EDGE).child(head).child(code)
}

fn step_glyph(status: Status, t: &tok::Theme) -> El {
    let slot = el().size(14.0, 14.0).center();
    match status {
        Status::Running => slot.child(el().size(7.0, 7.0).radius(3.5).fill(ACCENT.mix(Color::WHITE, 0.2)).cycle(PULSE)),
        Status::Done => slot.child(icon(&glyphs::CHECK_SMALL, 13.0).color(green())),
        Status::Failed => slot.child(icon(&glyphs::CLOSE, 10.0).color(red())),
    }
    .key("status")
    .label(match status {
        Status::Running => "Running",
        Status::Done => "Done",
        Status::Failed => "Failed",
    })
    .color(t.text)
}

fn agent<S: 'static>(token: u64, j: usize, a: &Agent, t: &tok::Theme, lens: Lens<S>) -> El {
    let running = a.steps.iter().any(|s| s.status == Status::Running);
    let failed = a.steps.iter().filter(|s| s.status == Status::Failed).count();
    let title = a.summary();
    let status = if running {
        Status::Running
    } else if failed > 0 {
        Status::Failed
    } else {
        Status::Done
    };
    let head = row()
        .key("head")
        .w_full()
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .hover(Color::WHITE.alpha(0.04))
        .role("button")
        .label(title.clone())
        .expanded(a.open)
        .child(step_glyph(status, t))
        .child(text(title).font(SMALL_MEDIUM).color(t.text).single_line().truncate().grow(1.0).min_w(0.0))
        .child(chevron(a.open, t))
        .on_press(toggle(lens, token, j, None));
    let mut out = col().w_full().radius(10.0).continuous_corners().clip_rounded(10.0).fill(CARD).border(1.0, EDGE).child(head);
    if a.open {
        let mut steps = col().key("steps").w_full().pb(6.0);
        for (k, s) in a.steps.iter().enumerate() {
            steps = steps.child(step(token, j, k, s, t, lens));
        }
        out = out.child(el().w_full().h(1.0).fill(EDGE)).child(steps);
    }
    out
}

fn step<S: 'static>(token: u64, j: usize, k: usize, s: &Step, t: &tok::Theme, lens: Lens<S>) -> El {
    let verb = s.tool.verb();
    let line = format!("{verb} {}", s.target);
    let spans = vec![Span::new(0, verb.len()).weight(600.0), Span::new(verb.len() + 1, line.len()).mono().color(t.text_secondary)];
    let has_output = !s.output.is_empty();
    let mut head = row()
        .key("head")
        .w_full()
        .h(28.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .child(step_glyph(s.status, t))
        .child(rich(line.clone(), spans).font(SMALL).color(t.text).single_line().truncate().grow(1.0).min_w(0.0));
    if has_output {
        head = head
            .cursor(Cursor::Pointer)
            .hover(Color::WHITE.alpha(0.04))
            .role("button")
            .label(line)
            .expanded(s.open)
            .child(chevron(s.open, t))
            .on_press(toggle(lens, token, j, Some(k)));
    }
    let mut out = col().key(format!("s{}", s.id)).w_full().child(head);
    if s.open && has_output {
        out = out.child(
            el().key("output")
                .w_full()
                .pl(32.0)
                .pr(10.0)
                .pb(6.0)
                .child(
                    el().w_full()
                        .padding(8.0)
                        .radius(6.0)
                        .fill(CODE_BG)
                        .child(text(s.output.clone()).font(MONO_SMALL).color(t.text_secondary).wrap().selectable(true)),
                ),
        );
    }
    out
}

fn diff<S: 'static>(token: u64, j: usize, d: &Diff, t: &tok::Theme, lens: Lens<S>) -> El {
    let patch = d.patch.clone();
    let path = d.path.clone();
    let title = row()
        .key("title")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .gap(6.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .role("button")
        .label(format!("{path}, {} added, {} removed", d.added, d.removed))
        .expanded(d.open)
        .child(chevron(d.open, t))
        .child(icon(&glyphs::FILE_CODE, 13.0).color(t.text_secondary))
        .child(text(path).font(MONO_SMALL).color(t.text).single_line().truncate_head().shrink().min_w(0.0))
        .child(text(format!("+{}", d.added)).font(MONO_SMALL).color(green()))
        .child(text(format!("−{}", d.removed)).font(MONO_SMALL).color(red()))
        .on_press(toggle(lens, token, j, None));
    let apply = if d.applied {
        word_button::<S>("apply", "Applied", Color::WHITE.alpha(0.06), green(), false, |_, _| {})
    } else {
        word_button::<S>("apply", "Apply", ACCENT, Color::WHITE, true, move |s: &mut S, _: &mut Tx| {
            if let Some(r) = lens(s).reply_mut(token) {
                r.apply_diff(j);
            }
        })
    };
    let head = row()
        .key("head")
        .w_full()
        .h(34.0)
        .pl(10.0)
        .pr(5.0)
        .gap(6.0)
        .center_y()
        .fill(Color::WHITE.alpha(0.04))
        .child(title)
        .child(apply)
        .child(icon_button::<S>("copy", &glyphs::COPY, "Copy diff", t, move |_, tx| tx.copy(patch.clone())));
    let mut out = col().w_full().radius(10.0).continuous_corners().clip_rounded(10.0).fill(CODE_BG).border(1.0, EDGE).child(head);
    if d.open {
        let body = Markdown::new(format!("```diff\n{}\n```", d.patch.trim_end())).render(&md_style(t).code_margin(0.0, 0.0));
        out = out.child(el().key("body").w_full().child(body));
    }
    out
}

fn question<S: 'static>(token: u64, j: usize, q: &Question, t: &tok::Theme, lens: Lens<S>) -> El {
    let open = q.answered.is_none();
    let mut card = col()
        .w_full()
        .gap(8.0)
        .padding(12.0)
        .radius(12.0)
        .continuous_corners()
        .fill(CARD)
        .border(1.0, ACCENT.alpha(if open { 0.45 } else { 0.2 }))
        .child(text(q.prompt.clone()).key("prompt").font(TextStyle::new(13.5, 600.0).lh(20.0)).color(t.text).wrap());
    if q.multi && open {
        card = card.child(text("Pick any").key("hint").font(SMALL).color(t.text_secondary));
    }
    let mut options = col().key("options").w_full().gap(4.0);
    for (i, option) in q.options.iter().enumerate() {
        let picked = q.picked.contains(&i);
        let mark = if q.multi {
            el().size(14.0, 14.0).radius(4.0)
        } else {
            el().size(14.0, 14.0).radius(7.0)
        };
        let mark = if picked {
            mark.fill(ACCENT).center().child(icon(&glyphs::CHECK_SMALL, 11.0).color(Color::WHITE))
        } else {
            mark.border(1.5, Color::WHITE.alpha(0.3))
        };
        let mut o = row()
            .key(format!("o{i}"))
            .w_full()
            .min_h(32.0)
            .px(10.0)
            .py(6.0)
            .gap(10.0)
            .center_y()
            .radius(8.0)
            .continuous_corners()
            .fill(if picked { ACCENT.alpha(0.18) } else { Color::WHITE.alpha(0.03) })
            .role(if q.multi { "checkbox" } else { "radio" })
            .label(option.clone())
            .checked(picked)
            .child(mark)
            .child(text(option.clone()).font(BODY).color(t.text).wrap().grow(1.0).min_w(0.0));
        if open {
            o = o.hover(Color::WHITE.alpha(0.06)).hover_alone().cursor(Cursor::Pointer).on_press(move |s: &mut S| lens(s).pick(token, j, i));
        } else {
            o = o.opacity(if picked { 1.0 } else { 0.5 });
        }
        options = options.child(o);
    }
    card = card.child(options);
    if let Some(answer) = &q.answered {
        return card.child(text(format!("Answered: {answer}")).key("answered").font(SMALL).color(t.text_secondary).wrap());
    }
    let ready = !q.picked.is_empty() || !q.other.text.trim().is_empty();
    let answer = move |s: &mut S, tx: &mut Tx| {
        if lens(s).answer(token, j).is_some() {
            tx.scroll_to_bottom("messages");
        }
    };
    let field = text_input(&q.other, SMALL.lh(18.0))
        .key("other")
        .label("Something else")
        .w_full()
        .color(t.text)
        .caret(t.text, ACCENT.alpha(0.35))
        .placeholder("Something else…", t.text_secondary.alpha(0.8))
        .on_edit(move |s: &mut S| lens(s).question_other(token, j))
        .on_submit(answer);
    let other = row()
        .key("other-row")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(el().grow(1.0).min_w(0.0).h(28.0).px(8.0).center_y().radius(6.0).fill(Color::WHITE.alpha(0.04)).border(1.0, EDGE).child(field))
        .child(word_button::<S>("answer", "Answer", ACCENT, Color::WHITE, ready, answer));
    card.child(other)
}

fn error<S: 'static>(token: u64, message: &str, t: &tok::Theme, lens: Lens<S>) -> El {
    row()
        .w_full()
        .gap(10.0)
        .padding(12.0)
        .radius(10.0)
        .continuous_corners()
        .fill(red().alpha(0.10))
        .border(1.0, red().alpha(0.35))
        .role("alert")
        .child(el().size(16.0, 18.0).center().child(icon(&glyphs::CIRCLE_X, 15.0).color(red())))
        .child(
            col().grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(text("Something went wrong").font(SMALL_MEDIUM).color(t.text))
                .child(text(message.to_owned()).font(SMALL).color(t.text_secondary).wrap()),
        )
        .child(word_button::<S>("retry", "Retry", Color::WHITE.alpha(0.10), t.text, true, move |s: &mut S, tx: &mut Tx| {
            if lens(s).retry(token).is_some() {
                tx.scroll_to_bottom("messages");
            }
        }))
}

/// Under a finished reply, shown on hover: Copy and Retry; "Stopped" when the user stopped it.
fn footer<S: 'static>(r: &Reply, t: &tok::Theme, lens: Lens<S>) -> El {
    let token = r.token;
    let copy = r.copy_text();
    let mut actions = row().key("actions").gap(2.0).center_y().group_opacity(0.0, 1.0);
    if !copy.is_empty() {
        actions = actions.child(icon_button::<S>("copy", &glyphs::COPY, "Copy reply", t, move |_, tx| tx.copy(copy.clone())));
    }
    if r.state != State::Failed {
        actions = actions.child(icon_button::<S>("retry", &crate::icons::RETRY, "Retry", t, move |s: &mut S, _: &mut Tx| {
            lens(s).retry(token);
        }));
    }
    let mut out = row().key("footer").h(24.0).gap(8.0).center_y().ml(-4.0);
    if r.state == State::Stopped {
        out = out.child(text("Stopped").key("stopped").font(SMALL).color(t.text_secondary).pl(4.0));
    }
    out.child(actions)
}

/// The send button's place while a reply streams: Stop.
pub fn stop_button<S: 'static>(lens: Lens<S>) -> El {
    el().key("stop")
        .size(28.0, 28.0)
        .radius(8.0)
        .continuous_corners()
        .center()
        .fill(Color::WHITE.alpha(0.14))
        .hover(Color::WHITE.alpha(0.10))
        .press(Color::BLACK.alpha(0.12))
        .press_scale(0.94)
        .cursor(Cursor::Pointer)
        .label("Stop")
        .shortcut_hint("Esc")
        .child(el().size(10.0, 10.0).radius(2.5).fill(Color::WHITE))
        .on_press(move |s: &mut S| lens(s).stop(Instant::now()))
}

#[cfg(test)]
mod tests {
    use rune::{Cx, Env, Runtime};

    use super::*;
    use crate::chat::{self, Insets};
    use crate::mock_model;
    use crate::reply::Event;

    struct W {
        chat: Chat,
    }

    fn window(s: &W, _: &mut Cx<W>) -> El {
        chat::panel(&s.chat, "panel", |s: &mut W| &mut s.chat, None)
    }

    fn keys(rt: &Runtime<W>) -> Vec<String> {
        rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.key).collect()
    }

    /// The fake model's whole showcase, then a frame drawn, with every part open.
    fn showcase(open: bool) -> Runtime<W> {
        let mut chat = Chat::new(None);
        chat.place(Insets::default(), [1440.0, 900.0]);
        chat.open();
        let mut rt = Runtime::new(W { chat }, window, (1440.0, 900.0), Env::new(true, 2.0));
        rt.update(|s, _| {
            let id = chat::tests::sent(&mut s.chat, "hello");
            let token = s.chat.ask(id).unwrap();
            for (_, event) in mock_model::script("hello") {
                s.chat.reply_event(token, event, Instant::now());
            }
            let r = s.chat.reply_mut(token).unwrap();
            if open {
                for j in 0..r.parts.len() {
                    if !matches!(r.parts[j], Part::Agent(_) | Part::Diff(_)) {
                        r.toggle(j, None);
                    }
                }
            }
        });
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    #[test]
    fn every_kind_of_part_draws_its_component() {
        let keys = keys(&showcase(true));
        for key in ["head", "body", "code-head", "steps", "s1", "apply", "options", "o2", "other", "answer", "footer"] {
            assert!(keys.iter().any(|k| k == key), "{key} in {keys:?}");
        }
        assert!(!keys.iter().any(|k| k == "stop"), "finished: Send again, not Stop");
    }

    fn settle(rt: &mut Runtime<W>, frames: usize) {
        for _ in 0..frames {
            rt.frame(1.0 / 60.0);
        }
    }

    fn has(rt: &Runtime<W>, key: &str) -> bool {
        keys(rt).iter().any(|k| k == key)
    }

    /// A long conversation, landed at its bottom; the thread's centre.
    fn long_chat() -> (Runtime<W>, (f32, f32)) {
        let mut chat = Chat::new(None);
        chat.place(Insets::default(), [1440.0, 900.0]);
        chat.open();
        let mut rt = Runtime::new(W { chat }, window, (1440.0, 900.0), Env::new(true, 2.0));
        for k in 0..6 {
            rt.update(|s, _| {
                let id = chat::tests::sent(&mut s.chat, &format!("code {k}"));
                let token = s.chat.ask(id).unwrap();
                for (_, event) in mock_model::script("code") {
                    s.chat.reply_event(token, event, Instant::now());
                }
            });
            settle(&mut rt, 20);
        }
        settle(&mut rt, 60);
        let m = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("messages")).unwrap();
        (rt, (m.rect[0] + m.rect[2] / 2.0, m.rect[1] + m.rect[3] / 2.0))
    }

    #[test]
    fn the_thread_scrolls_back_and_new_content_keeps_the_place_with_a_way_back_to_the_latest() {
        let (mut rt, (x, y)) = long_chat();
        assert_eq!(rt.scroll_at_bottom("messages"), Some(true), "lands on the latest");
        assert!(!has(&rt, "to-latest"));
        rt.wheel(x, y, 400.0);
        settle(&mut rt, 20);
        assert_eq!(rt.scroll_at_bottom("messages"), Some(false), "scrolled back");
        assert!(has(&rt, "to-latest"), "a way back");
        // A reply streams in meanwhile: the place is kept.
        let mut token = 0;
        rt.update(|s, _| {
            let id = chat::tests::sent(&mut s.chat, "more");
            token = s.chat.ask(id).unwrap();
        });
        for k in 0..20 {
            rt.update(|s, _| s.chat.reply_event(token, Event::Text(format!("word {k} ")), Instant::now()));
            settle(&mut rt, 3);
        }
        assert_eq!(rt.scroll_at_bottom("messages"), Some(false), "kept its place");
        let b = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("to-latest")).expect("still offered");
        let (bx, by) = (b.rect[0] + b.rect[2] / 2.0, b.rect[1] + b.rect[3] / 2.0);
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        settle(&mut rt, 90);
        assert_eq!(rt.scroll_at_bottom("messages"), Some(true), "back at the latest");
        assert!(!has(&rt, "to-latest"));
    }

    #[test]
    fn every_text_field_has_a_name_a_screen_reader_can_say() {
        use crate::quick_input::{self, QuickInput};
        let mut q = QuickInput::new(None);
        q.chat.place(Insets::default(), [1440.0, 900.0]);
        q.open_chat();
        let id = chat::tests::sent(&mut q.chat, "ask");
        let token = q.chat.ask(id).unwrap();
        for (_, event) in mock_model::script("ask") {
            q.chat.reply_event(token, event, Instant::now());
        }
        let mut rt = Runtime::new(q, quick_input::view, (1440.0, 900.0), Env::new(true, 2.0));
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let elements = rt.elements();
        for key in ["query", "draft", "other"] {
            let field = elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key} shown"));
            assert!(field.label.as_deref().is_some_and(|l| !l.trim().is_empty()), "{key} has no name");
        }
    }

    #[test]
    fn in_the_quick_window_the_wheel_scrolls_the_chat_back() {
        use crate::quick_input::{self, QuickInput};
        let mut q = QuickInput::new(None);
        q.chat.place(Insets::default(), [1440.0, 900.0]);
        q.open_chat();
        let mut rt = Runtime::new(q, quick_input::view, (1440.0, 900.0), Env::new(true, 2.0));
        for k in 0..6 {
            rt.update(|s, _| {
                let id = chat::tests::sent(&mut s.chat, &format!("code {k}"));
                let token = s.chat.ask(id).unwrap();
                for (_, event) in mock_model::script("code") {
                    s.chat.reply_event(token, event, Instant::now());
                }
            });
            for _ in 0..20 {
                rt.frame(1.0 / 60.0);
            }
        }
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        let m = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("messages")).unwrap();
        let (x, y) = (m.rect[0] + m.rect[2] / 2.0, m.rect[1] + m.rect[3] / 2.0);
        assert_eq!(rt.scroll_at_bottom("messages"), Some(true));
        rt.pointer_move(x, y);
        // A mouse wheel's notches, as winit hands them over (lines, imprecise, no phase).
        for _ in 0..4 {
            rt.wheel_event(rune::WheelEvent::new(0.0, 3.0 * 40.0).at(x, y).phase(rune::GesturePhase::Moved).precise(false));
            for _ in 0..5 {
                rt.frame(1.0 / 60.0);
            }
        }
        assert_eq!(rt.scroll_at_bottom("messages"), Some(false), "the wheel at {x},{y} in {:?}", m.rect);
    }

    #[test]
    fn at_the_latest_it_follows_a_streaming_reply_and_offers_no_way_back() {
        let (mut rt, _) = long_chat();
        let mut token = 0;
        rt.update(|s, _| {
            let id = chat::tests::sent(&mut s.chat, "more");
            token = s.chat.ask(id).unwrap();
        });
        for k in 0..40 {
            rt.update(|s, _| s.chat.reply_event(token, Event::Text(format!("word {k} ")), Instant::now()));
            settle(&mut rt, 2);
            assert!(!has(&rt, "to-latest"), "frame {k}: following, no button");
        }
        settle(&mut rt, 60);
        assert_eq!(rt.scroll_at_bottom("messages"), Some(true));
    }

    #[test]
    fn while_it_streams_send_is_stop_and_an_error_offers_retry() {
        let mut chat = Chat::new(None);
        chat.place(Insets::default(), [1440.0, 900.0]);
        chat.open();
        let mut rt = Runtime::new(W { chat }, window, (1440.0, 900.0), Env::new(true, 2.0));
        rt.update(|s, _| {
            let id = chat::tests::sent(&mut s.chat, "hi");
            s.chat.ask(id);
        });
        rt.frame(1.0 / 60.0);
        let k = keys(&rt);
        assert!(k.iter().any(|k| k == "stop") && k.iter().any(|k| k == "waiting"), "{k:?}");
        rt.update(|s, _| {
            let id = chat::tests::sent(&mut s.chat, "meanwhile");
            s.chat.ask(id);
        });
        rt.frame(1.0 / 60.0);
        assert!(keys(&rt).iter().any(|k| k == "queued"), "sent while it streams: queued");
        rt.update(|s, _| {
            s.chat.stop(Instant::now());
            s.chat.stop(Instant::now());
            let id = chat::tests::sent(&mut s.chat, "error");
            let token = s.chat.ask(id).unwrap();
            for (_, event) in mock_model::script("error") {
                s.chat.reply_event(token, event, Instant::now());
            }
        });
        rt.frame(1.0 / 60.0);
        assert!(keys(&rt).iter().any(|k| k == "retry"));
    }
}
