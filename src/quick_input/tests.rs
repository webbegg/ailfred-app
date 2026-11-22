use super::labels::placeholder;
use super::*;

fn close_to(a: Color, b: Color) -> bool {
    [a.r - b.r, a.g - b.g, a.b - b.b, a.a - b.a].iter().all(|d| d.abs() < 0.002)
}

#[test]
fn colours_are_the_srgb_ones_in_linear() {
    assert!(close_to(ACCENT, rune::color::hex("#D97757").unwrap()), "{ACCENT:?}");
    assert!(close_to(FROST, rune::color::hex("#1E1E1E").unwrap().alpha(FROST.a)), "{FROST:?}");
}

#[test]
fn starts_closed_below_the_screen_so_it_slides_in_from_outside() {
    let s = QuickInput::new(None);
    assert!(!s.is_open());
    assert_eq!(pose(false, false), Pose { dy: TUCK, alpha: 1.0, scale: 1.0 });
    const { assert!(TUCK >= PANEL_H + MARGIN + PAD, "fully below the bottom edge, shadow included") };
}

#[test]
fn opening_rises_to_rest_opaque_with_the_field_focused() {
    let mut s = QuickInput::new(None);
    s.open();
    assert!(s.is_open());
    assert!(s.query.focused, "the field takes the keys");
    assert_eq!(pose(true, false), Pose { dy: 0.0, alpha: 1.0, scale: 1.0 });
}

fn tiny() -> chat::Image {
    chat::Image::from_rgba(1, 1, vec![0; 4])
}

#[test]
fn closing_reports_whether_it_was_open_and_keeps_the_text() {
    let mut s = QuickInput::new(None);
    assert!(!s.close(), "already closed");
    s.open();
    s.query = TextEdit::new("draft");
    assert!(s.close());
    assert!(!s.is_open());
    assert_eq!(s.query.text, "draft");
}

#[test]
fn submitting_takes_trimmed_text_and_clears_the_field() {
    let mut s = QuickInput::new(None);
    s.open();
    s.query = TextEdit::new("  hello there \n");
    let (text, shots) = s.take_submission().unwrap();
    assert_eq!((text.as_str(), shots.len()), ("hello there", 0));
    assert_eq!(s.query.text, "");
}

#[test]
fn submitting_nothing_does_nothing() {
    let mut s = QuickInput::new(None);
    s.query = TextEdit::new("   ");
    assert!(s.take_submission().is_none());
    assert_eq!(s.query.text, "   ");
}

#[test]
fn send_is_ready_with_text_or_a_capture() {
    let mut s = QuickInput::new(None);
    assert!(!s.send_ready());
    s.query = TextEdit::new("  ");
    assert!(!s.send_ready(), "blank text isn't something to send");
    s.query = TextEdit::new("hi");
    assert!(s.send_ready());
    s.query = TextEdit::new("");
    s.attach(1, vec![1], tiny());
    assert!(s.send_ready());
}

#[test]
fn captures_attach_and_go_with_the_submission() {
    let mut s = QuickInput::new(None);
    s.open();
    s.attach(1, vec![1], tiny());
    s.attach(2, vec![2], tiny());
    assert_eq!(s.ids(), vec![1, 2]);
    s.query = TextEdit::new("what is this?");
    let (text, shots) = s.take_submission().unwrap();
    assert_eq!(text, "what is this?");
    assert_eq!(shots.iter().map(|a| a.png.clone()).collect::<Vec<_>>(), vec![vec![1], vec![2]]);
    assert!(s.ids().is_empty(), "attachments clear on submit");
}

#[test]
fn the_same_area_again_replaces_its_capture_in_place() {
    let mut s = QuickInput::new(None);
    s.attach(1, vec![1], tiny());
    s.attach(2, vec![2], tiny());
    s.attach(1, vec![9], tiny());
    assert_eq!(s.ids(), vec![1, 2], "no second chip, same place");
    assert_eq!(s.shots[0].png, vec![9], "with the marked-up image");
}

#[test]
fn a_chips_x_removes_only_that_capture() {
    let mut s = QuickInput::new(None);
    s.attach(1, vec![1], tiny());
    s.attach(2, vec![2], tiny());
    assert!(s.remove(1));
    assert_eq!(s.ids(), vec![2]);
    assert!(!s.remove(1), "already gone");
}

#[test]
fn backspace_in_an_empty_field_removes_the_last_capture() {
    let mut s = QuickInput::new(None);
    assert_eq!(s.backspace_empty(), None, "nothing to remove");
    s.attach(1, vec![1], tiny());
    s.attach(2, vec![2], tiny());
    assert_eq!(s.backspace_empty(), Some(2));
    assert_eq!(s.ids(), vec![1]);
    assert_eq!(s.backspace_empty(), Some(1));
    assert!(s.ids().is_empty());
}

#[test]
fn an_image_pasted_into_the_input_takes_the_next_id_and_text_is_left_to_the_field() {
    use crate::chat::tests::{a_png, clip};
    let mut s = QuickInput::new(None);
    assert_eq!(s.take_paste(&clip("just text", None)), vec![]);
    assert_eq!(s.take_paste(&clip("caption", Some(a_png()))), vec![(1, chat::Pasted::Png(a_png()))], "the image, not its text");
    assert_eq!(s.take_paste(&clip("", Some(a_png()))).first().map(|(id, _)| *id), Some(2));
}

#[test]
fn a_chips_image_opens_enlarged_and_esc_closes_it_before_the_input() {
    let mut s = QuickInput::new(None);
    s.open();
    s.attach(1, vec![1], tiny());
    s.attach(2, vec![2], tiny());
    assert!(s.preview_image().is_none());
    s.open_preview(2);
    assert!(s.preview_image().is_some());
    assert_eq!(s.escape(), Escape::Preview, "the enlarged image closes first");
    assert!(s.preview_image().is_none());
    assert_eq!(s.ids(), vec![1, 2], "its capture stays attached");
    assert!(s.is_open(), "the input stays");
    assert_eq!(s.escape(), Escape::Close, "then the input closes");
}

#[test]
fn removing_its_capture_or_closing_closes_the_enlarged_image() {
    let mut s = QuickInput::new(None);
    s.open();
    s.attach(1, vec![1], tiny());
    s.attach(2, vec![2], tiny());
    s.open_preview(2);
    assert!(s.remove(1), "another one");
    assert!(s.preview_image().is_some());
    assert_eq!(s.backspace_empty(), Some(2));
    assert!(s.preview_image().is_none(), "its own");
    assert_eq!(s.escape(), Escape::Close, "nothing enlarged left to close first");
    s.open();
    s.attach(3, vec![3], tiny());
    s.open_preview(3);
    s.close();
    assert!(s.preview_image().is_none());
}

#[test]
fn the_chats_enlarged_image_shows_in_the_shared_window() {
    let mut s = QuickInput::new(None);
    s.chat.attach(5, vec![5], tiny());
    s.chat.open_preview(5);
    assert!(s.preview_image().is_some());
    s.close_preview();
    assert!(s.preview_image().is_none());
}

#[test]
fn closing_discards_the_captures_and_keeps_the_text() {
    let mut s = QuickInput::new(None);
    s.open();
    s.attach(1, vec![1], tiny());
    s.query = TextEdit::new("draft");
    s.close();
    assert!(s.ids().is_empty());
    assert_eq!(s.query.text, "draft");
}

#[test]
fn many_chips_go_compact() {
    assert!(!compact(INPUT_FULL_CHIPS, INPUT_FULL_CHIPS));
    assert!(compact(INPUT_FULL_CHIPS + 1, INPUT_FULL_CHIPS));
    let room = PANEL_W - 2.0 * ROW_PAD;
    assert!(INPUT_FULL_CHIPS as f32 * (THUMB.0 + 8.0) <= room, "the full ones fit on one line");
}

#[test]
fn chips_sit_in_a_strip_above_the_field_which_stays_put() {
    assert_eq!(strip_h(0), 0.0, "no chips, no strip");
    assert_eq!(strip_h(1), STRIP_TOP + THUMB.1);
    assert_eq!(strip_h(INPUT_FULL_CHIPS), STRIP_TOP + THUMB.1);
    assert_eq!(strip_h(INPUT_FULL_CHIPS + 1), STRIP_TOP + THUMB_COMPACT.1, "smaller when many");
    let mut s = QuickInput::new(None);
    let r = [10.0, 500.0, PANEL_W, PANEL_H];
    s.set_rect(r);
    assert_eq!(s.panel_rect(), r);
    s.attach(1, vec![1], tiny());
    let grow = strip_h(1);
    assert_eq!(s.panel_rect(), [10.0, 500.0 - grow, PANEL_W, PANEL_H + grow], "grows upward");
    s.open();
    s.set_tucked(true);
    assert_eq!(s.current_pose().dy, TUCK + grow, "tucked, the strip too goes out of the screen");
}

#[test]
fn the_hint_says_what_the_field_is_for() {
    assert_eq!(placeholder(0), "Ask anything…");
    assert_eq!(placeholder(1), "Ask about this screenshot…");
    assert_eq!(placeholder(3), "Ask about these screenshots…");
    assert_eq!(hint(0, Some("⇧⌘Space")), "Ask anything…  ⇧⌘Space to take a screenshot");
    assert_eq!(hint(0, None), "Ask anything…", "no capture shortcut set");
    assert_eq!(hint(1, Some("⇧⌘Space")), "Ask about this screenshot…");
}

#[test]
fn a_shortcut_reads_as_settings_shows_it() {
    let caps = |s: &str| s.parse::<rune::Shortcut>().unwrap().keycaps();
    assert_eq!(chord_symbols(&caps("shift+super+space")), "⇧⌘Space");
    assert_eq!(chord_symbols(&caps("super+shift+4")), "⇧⌘4", "in the Mac's order");
    assert_eq!(chord_symbols(&caps("super+slash")), "⌘/");
    assert_eq!(chord_symbols(&caps("alt+f5")), "⌥F5");
}

#[test]
fn a_capture_alone_can_be_submitted() {
    let mut s = QuickInput::new(None);
    s.attach(1, vec![0], tiny());
    let (text, shots) = s.take_submission().unwrap();
    assert_eq!((text.as_str(), shots.len()), ("", 1));
}

#[test]
fn tucking_at_the_bottom_slides_it_out_of_the_screen_and_back() {
    let mut s = QuickInput::new(None);
    s.open();
    s.set_tucked(true);
    assert_eq!(pose(s.open, s.tucked), Pose { dy: TUCK, alpha: 0.0, scale: 1.0 }, "below the screen");
    s.set_tucked(false);
    assert_eq!(pose(s.open, s.tucked).dy, 0.0);
}

#[test]
fn a_closed_input_stays_closed_when_tucked_and_reopens_untucked() {
    let mut s = QuickInput::new(None);
    s.set_tucked(true);
    assert_eq!(pose(s.open, s.tucked).dy, TUCK);
    s.open();
    assert!(!s.tucked);
}

#[test]
fn over_a_dock_it_still_rises_from_the_screens_edge() {
    let mut s = QuickInput::new(None);
    s.at_bottom(70.0);
    assert_eq!(s.current_pose().dy, TUCK + 70.0, "below the screen, past the Dock");
    s.open();
    assert_eq!(s.current_pose().dy, 0.0, "rests above the Dock");
    s.set_tucked(true);
    assert_eq!(s.current_pose().dy, TUCK + 70.0, "tucks out of the screen too");
    s.close();
    assert_eq!(s.current_pose().dy, TUCK + 70.0);
}

#[test]
fn at_the_bottom_the_panel_is_centred_above_the_gap_and_dock() {
    assert_eq!(bottom_panel([1920.0, 1080.0], 70.0), [650.0, 1080.0 - 70.0 - MARGIN - PANEL_H, PANEL_W, PANEL_H]);
    assert_eq!(bottom_panel([1920.0, 1080.0], 0.0)[1], 1080.0 - MARGIN - PANEL_H);
}

#[test]
fn the_dock_inset_is_the_visible_areas_bottom_gap() {
    let frame = [0.0, 0.0, 1512.0, 982.0];
    assert_eq!(dock_inset(frame, [0.0, 70.0, 1512.0, 879.0]), 70.0, "Dock at the bottom");
    assert_eq!(dock_inset(frame, [0.0, 0.0, 1512.0, 949.0]), 0.0, "hidden Dock: only the menu bar");
    assert_eq!(dock_inset(frame, [64.0, 0.0, 1448.0, 949.0]), 0.0, "Dock on the left");
    // A second display above the first: AppKit y is global.
    assert_eq!(dock_inset([0.0, 982.0, 1920.0, 1080.0], [0.0, 1052.0, 1920.0, 985.0]), 70.0);
}

fn sent(text: &str, shot: Option<u64>) -> QuickInput {
    let mut s = QuickInput::new(None);
    s.set_rect(bottom_panel([1512.0, 982.0], 0.0));
    s.open();
    s.query = TextEdit::new(text);
    if let Some(id) = shot {
        s.attach(id, vec![1, 2, 3], tiny());
    }
    s
}

#[test]
fn closed_it_hides_past_the_windows_bottom_whatever_screen_it_was_placed_for() {
    let top = |s: &QuickInput, window_h: f32| {
        let [_, bottom] = s.pins();
        window_h - bottom - s.panel_rect()[3] + s.current_pose().dy
    };
    // Never placed (launch), or placed for a smaller display than it shows on.
    let fresh = QuickInput::new(None);
    let mut small = QuickInput::new(None);
    small.at_bottom(70.0);
    small.set_rect(bottom_panel([1440.0, 900.0], 70.0));
    small.attach(1, vec![1], tiny());
    for s in [&fresh, &small] {
        for window_h in [900.0, 1080.0, 1440.0] {
            assert!(top(s, window_h) >= window_h + PAD, "closed: below a {window_h}-tall window");
        }
    }
    // Open, it's at its rect.
    let mut s = QuickInput::new(None);
    s.at_bottom(70.0);
    s.set_rect(bottom_panel([1920.0, 1080.0], 70.0));
    s.attach(1, vec![1], tiny());
    s.open();
    assert_eq!([s.pins()[0], top(&s, 1080.0)], [s.panel_rect()[0], s.panel_rect()[1]]);
}

#[test]
fn sending_presses_the_whole_panel_in_then_goes_once_its_down() {
    let mut s = sent("what is this?", None);
    let press = s.press().unwrap();
    assert_eq!(s.current_pose().scale, PRESSED_SCALE, "pressed in");
    assert!(!s.morphing && s.is_open(), "not sent yet");
    assert_eq!(s.press(), None, "Enter again meanwhile sends nothing more");
    s.release(press);
    assert!(s.morphing && s.chat.is_open(), "sent: it becomes the chat");
    assert_eq!(s.current_pose().scale, 1.0);
    assert_eq!(s.press(), None, "nothing left to send");
    // Closed while pressed: nothing goes.
    let mut s = sent("draft", None);
    let press = s.press().unwrap();
    s.close();
    s.release(press);
    assert!(!s.morphing && !s.chat.is_open());
    assert_eq!(QuickInput::new(None).press(), None, "empty: no press");
}

#[test]
fn a_release_goes_with_its_own_press_only() {
    let mut s = sent("a", None);
    let first = s.press().unwrap();
    s.close();
    s.open();
    let second = s.press().unwrap();
    s.release(first);
    assert!(!s.morphing && s.is_open(), "the first press's timer doesn't send the second early");
    assert_eq!(s.current_pose().scale, PRESSED_SCALE, "still pressed");
    s.release(second);
    assert!(s.morphing);
}

#[test]
fn cmd_n_before_the_chat_lands_keeps_what_goes_into_the_new_input() {
    let mut s = sent("hi", Some(1));
    s.send();
    s.at_bottom(0.0);
    s.open();
    assert!(s.query.text.is_empty() && s.ids().is_empty(), "the new input starts empty");
    assert_eq!(s.panel_rect()[3], PANEL_H, "not grown by the sent images");
    s.query = TextEdit::new("next");
    s.attach(99, vec![9], tiny());
    s.land();
    assert_eq!((s.query.text.as_str(), s.ids()), ("next", vec![99]), "the landing doesn't clear it");
}

#[test]
fn an_image_still_decoding_when_the_input_closes_or_sends_is_dropped() {
    use crate::chat::tests::{a_png, clip};
    let mut s = QuickInput::new(None);
    s.open();
    let id = s.take_paste(&clip("", Some(a_png())))[0].0;
    s.close();
    s.open();
    s.pasted(id, vec![1], tiny());
    assert!(s.ids().is_empty(), "not into the next input");
    let id = s.take_paste(&clip("", Some(a_png())))[0].0;
    s.pasted(id, vec![1], tiny());
    assert_eq!(s.ids(), vec![id], "the same input: it joins");
    let late = s.take_paste(&clip("", Some(a_png())))[0].0;
    s.query = TextEdit::new("go");
    s.send();
    s.land();
    s.open();
    s.pasted(late, vec![1], tiny());
    assert!(s.ids().is_empty(), "not after it sent");
}

#[test]
fn the_log_never_holds_what_was_typed() {
    let line = sent_log("my secret question", &[vec![1, 2, 3]]);
    assert!(!line.contains("secret"), "{line}");
    assert!(line.contains("18 chars") && line.contains("1 capture") && line.contains("3 bytes"), "{line}");
}

#[test]
fn a_new_input_opened_before_the_chat_lands_keeps_the_keys() {
    let mut s = sent("hi", None);
    s.send();
    s.open();
    s.land();
    assert!(s.query.focused && !s.chat.draft_focused());
}

#[test]
fn the_panel_rests_at_its_rect() {
    let mut s = QuickInput::new(None);
    let r = [10.0, 20.0, 500.0, PANEL_H];
    s.set_rect(r);
    assert_eq!(s.rect, r);
}

#[test]
fn sending_hands_the_inputs_panel_to_the_chat_which_keeps_its_row_until_it_lands() {
    let mut s = sent("what is this?", Some(4));
    let panel = s.input_key();
    let (id, text, pngs) = s.send().expect("something to send");
    assert_eq!((text.as_str(), pngs), ("what is this?", vec![vec![1, 2, 3]]));
    assert!(!s.is_open() && s.chat.is_open());
    assert_eq!(s.chat_key(), panel, "the same element: it reshapes into the chat");
    assert_ne!(s.input_key(), panel, "the next input gets the other");
    assert!(s.morphing && !s.chat.landed());
    assert_eq!((s.query.text.as_str(), s.ids()), ("what is this?", vec![4]), "the row shows what was sent while it fades");
    assert!(!s.query.focused && s.chat.draft_focused(), "the chat's field has the keys");
    s.land();
    assert!(!s.morphing && s.chat.landed());
    assert_eq!((s.query.text.as_str(), s.ids().len()), ("", 0), "the next input starts empty");
    s.chat.close();
    s.open();
    s.query = TextEdit::new("again");
    assert_eq!(s.send().map(|m| m.0), Some(id + 1));
}

#[test]
fn with_the_chat_in_its_window_sending_closes_the_input_and_leaves_the_message_to_the_host() {
    let mut s = sent("first", None);
    assert!(s.send().is_some());
    let mut chat = s.take_chat();
    assert!(chat.windowed() && chat.landed(), "handed over landed, mid-flight or not");
    assert!(!s.chat.is_open() && !s.morphing);
    assert_eq!(chat.reply_to(0).map(|r| r.prompt.as_str()), Some("first"), "the conversation went with it");
    s.open();
    s.query = TextEdit::new("more");
    let (id, text, _) = s.send().expect("sent");
    assert_eq!((id, text.as_str()), (1, "more"));
    assert!(!s.is_open() && !s.morphing && !s.chat.is_open(), "the input slides down, no side panel");
    assert!(s.chat.reply_to(1).is_none(), "the message isn't the side panel's");
    chat.stop(std::time::Instant::now());
    s.put_chat(chat);
    assert!(!s.chat.windowed() && !s.chat.is_open());
    s.open();
    s.query = TextEdit::new("side");
    assert!(s.send().is_some());
    assert!(s.chat.is_open() && s.chat.reply_to(2).is_some(), "back to the side panel");
}

#[test]
fn sending_with_the_chat_open_adds_to_it_and_the_input_slides_down() {
    let mut s = sent("more", None);
    s.chat.open();
    let chat = s.chat_key();
    assert!(s.send().is_some());
    assert!(!s.is_open() && s.chat.is_open());
    assert_eq!(s.chat_key(), chat);
    assert!(!s.morphing && s.query.text.is_empty());
}

#[test]
fn nothing_typed_or_attached_sends_nothing() {
    let mut s = sent("  ", None);
    assert!(s.send().is_none());
    assert!(s.is_open() && !s.chat.is_open());
}

#[test]
fn the_shortcut_parses() {
    assert!(SHORTCUT.parse::<rune::Shortcut>().is_ok());
}
