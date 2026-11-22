use super::geometry::{GAP, PAD};
use super::images::image_file_png_within;
use super::*;

#[test]
fn closed_it_hides_past_the_windows_edge_whatever_screen_it_was_told() {
    // Never placed yet (launch): told 1440 × 900, shown on a wider display.
    for insets in [Insets::default(), Insets { top: 33.0, bottom: 70.0, right: 72.0 }] {
        let [top, right] = side_pins(insets);
        for window in [1440.0, 1512.0, 1920.0, 2560.0] {
            let left = window - right - CHAT_W;
            assert!(left + offset(false, insets) >= window + PAD, "closed: past a {window}-wide window");
            let [x, y, ..] = side_rect([window, 900.0], insets);
            assert_eq!([left, top], [x, y], "open: at its rect");
        }
    }
}

#[test]
fn insets_leave_the_menu_bar_and_the_dock() {
    let frame = [0.0, 0.0, 1512.0, 982.0];
    assert_eq!(insets(frame, [0.0, 70.0, 1512.0, 879.0]), Insets { top: 33.0, bottom: 70.0, right: 0.0 }, "Dock at the bottom");
    assert_eq!(insets(frame, [0.0, 0.0, 1440.0, 949.0]), Insets { top: 33.0, bottom: 0.0, right: 72.0 }, "Dock on the right");
    // A second display above the first: AppKit y is global.
    assert_eq!(insets([0.0, 982.0, 1920.0, 1080.0], [0.0, 982.0, 1920.0, 1055.0]), Insets { top: 25.0, bottom: 0.0, right: 0.0 });
}

#[test]
fn closed_it_waits_past_the_right_edge_and_opens_to_rest() {
    let insets = Insets { top: 33.0, bottom: 0.0, right: 72.0 };
    assert_eq!(offset(true, insets), 0.0);
    let [x, ..] = side_rect([1512.0, 982.0], insets);
    assert!(x + offset(false, insets) >= 1512.0 + PAD, "past the screen's edge, shadow included");
}

#[test]
fn at_rest_it_takes_the_right_edge_past_the_menu_bar_and_dock() {
    let insets = Insets { top: 33.0, bottom: 70.0, right: 0.0 };
    assert_eq!(side_rect([1512.0, 982.0], insets), [1512.0 - GAP - CHAT_W, 33.0 + GAP, CHAT_W, 982.0 - 33.0 - 70.0 - 2.0 * GAP]);
    let [x, _, w, _] = side_rect([1512.0, 982.0], Insets { right: 72.0, ..insets });
    assert_eq!(x + w, 1512.0 - 72.0 - GAP, "left of a Dock on the right");
}

fn msg(id: u64, text: &str) -> Message {
    Message { id, text: text.into(), images: Vec::new() }
}

#[test]
fn landing_gives_its_field_the_keys_though_it_lost_them_while_hidden() {
    let mut c = Chat::new(None);
    c.morph(Message { id: 0, text: "hi".into(), images: Vec::new() });
    // Its content is transparent while the panel reshapes, so the window's focus passes it
    // by and the field's model is told it's not focused.
    c.draft.focused = false;
    c.land();
    assert!(c.draft_focused(), "type the next message at once");
}

#[test]
fn the_inputs_panel_becomes_it_open_with_the_message_its_content_shown_once_landed() {
    let mut c = Chat::new(None);
    c.place(Insets::default(), [1512.0, 982.0]);
    c.morph(msg(0, "hi"));
    assert!(c.is_open(), "open at once: the input's panel springs to the side");
    assert!(!c.landed(), "its content waits while it reshapes");
    assert_eq!(c.messages.len(), 1);
    assert!(c.draft.focused, "type at once");
    c.land();
    assert!(c.landed());
    c.close();
    c.place(Insets::default(), [1512.0, 982.0]);
    assert!(c.landed(), "brought back later, it slides in whole");
}

#[test]
fn a_messages_captures_arrive_once_decoded() {
    let mut c = Chat::new(None);
    c.push(msg(3, "look"));
    c.push(msg(4, "and"));
    let image = Image { pixels: Pixels::new(), width: 2, height: 1 };
    c.set_images(3, vec![image]);
    assert_eq!(c.messages.iter().map(|m| m.images.len()).collect::<Vec<_>>(), vec![1, 0]);
    c.set_images(9, Vec::new());
}

#[test]
fn opening_focuses_its_field_and_closing_keeps_the_conversation() {
    let mut c = Chat::new(None);
    c.place(Insets::default(), [1512.0, 982.0]);
    c.push(msg(0, "hello"));
    c.draft.focused = false;
    c.open();
    assert!(c.is_open());
    assert!(c.draft.focused, "type at once");
    c.draft = TextEdit::new("draft");
    assert!(c.close());
    assert!(!c.close(), "already closed");
    assert_eq!((c.messages.len(), c.draft.text.as_str()), (1, "draft"));
}

#[test]
fn sending_appends_the_trimmed_draft_and_clears_it() {
    let mut c = Chat::new(None);
    c.draft = TextEdit::new("   ");
    assert!(c.send().is_none(), "nothing to send");
    assert!(c.messages.is_empty());
    c.draft = TextEdit::new("  and this?\n");
    assert!(c.send().is_some());
    assert_eq!(c.messages.last().map(|m| m.text.as_str()), Some("and this?"));
    assert_eq!(c.draft.text, "");
}

#[test]
fn captures_attach_to_the_draft_replace_in_place_and_go_with_the_next_message() {
    let mut c = Chat::new(None);
    c.attach(1, vec![1], tiny());
    c.attach(2, vec![2], tiny());
    c.attach(1, vec![9], tiny());
    assert_eq!(c.shot_ids(), vec![1, 2], "the same area again replaces its image in place");
    assert!(c.remove(2));
    assert!(!c.remove(2));
    let (id, pngs) = c.send().expect("a capture alone sends");
    assert_eq!(pngs, vec![vec![9]]);
    assert!(c.shot_ids().is_empty());
    assert_eq!(c.messages.last().map(|m| (m.id, m.text.as_str())), Some((id, "")));
}

pub(crate) fn sent(c: &mut Chat, text: &str) -> u64 {
    c.draft = TextEdit::new(text);
    let (id, _) = c.send().expect("sent");
    id
}

fn texts(r: &Reply) -> String {
    r.parts.iter().filter_map(|p| if let reply::Part::Text(t) = p { Some(t.as_str()) } else { None }).collect()
}

#[test]
fn in_its_window_it_stays_open_esc_leaves_it_and_cmd_n_clears_it_at_once() {
    let mut c = Chat::new(None);
    let id = sent(&mut c, "hi");
    let token = c.ask(id).unwrap();
    c.set_windowed(true);
    assert!(c.windowed() && c.is_open() && c.landed() && c.draft_focused());
    let esc = KeyEvent { key: Key::Escape, mods: rune::Mods::default() };
    c.stop(Instant::now());
    assert!(!c.field_key(&esc), "Esc doesn't close a window");
    c.attach(1, vec![1], tiny());
    c.draft = TextEdit::new("draft");
    c.clear();
    assert!(c.messages.is_empty() && c.replies.is_empty() && c.shots.is_empty() && c.draft.text.is_empty());
    c.reply_event(token, Event::Text("stale".into()), Instant::now());
    assert!(c.replies.is_empty());
    c.set_windowed(false);
    assert!(!c.windowed() && !c.is_open(), "back in the side panel, closed: it slides in");
    let id = sent(&mut c, "side");
    c.ask(id);
    c.stop(Instant::now());
    assert!(c.field_key(&esc), "Esc closes the side panel");
}

#[test]
fn a_message_typed_after_one_from_the_input_gets_an_id_of_its_own() {
    let mut c = Chat::new(None);
    c.push(Message { id: 0, text: "from the input".into(), images: Vec::new() });
    let typed = sent(&mut c, "typed");
    c.push(Message { id: 1, text: "from the input again".into(), images: Vec::new() });
    let again = sent(&mut c, "typed again");
    let ids: Vec<u64> = c.messages.iter().map(|m| m.id).collect();
    assert!(again != typed && again != 0 && again != 1, "{ids:?}");
}

#[test]
fn moved_between_the_panel_and_its_window_it_starts_at_the_latest() {
    let mut c = Chat::new(None);
    c.away = true;
    c.set_windowed(true);
    assert!(!c.away, "no stuck jump-to-latest button in the window");
    c.away = true;
    c.set_windowed(false);
    assert!(!c.away, "nor back in the side panel");
}

#[test]
fn docked_back_empty_it_offers_the_input_not_an_empty_panel() {
    assert_eq!(on_dock(true), Toggle::ShowChat);
    assert_eq!(on_dock(false), Toggle::ShowInput, "cleared with Cmd+N in its window");
}

#[test]
fn esc_while_a_reply_streams_stops_it_and_only_the_next_closes_the_side_panel() {
    let esc = KeyEvent { key: Key::Escape, mods: rune::Mods::default() };
    let mut c = Chat::new(None);
    c.open();
    let id = sent(&mut c, "hi");
    c.ask(id);
    assert!(c.streaming());
    assert!(c.field_key(&esc));
    assert!(!c.streaming() && c.is_open(), "stopped, still open");
    assert!(c.field_key(&esc), "the next asks the host to slide it out");
    let mut w = Chat::new(None);
    let id = sent(&mut w, "hi");
    w.ask(id);
    w.set_windowed(true);
    assert!(w.field_key(&esc) && !w.streaming() && w.is_open(), "stopped in the window");
    assert!(!w.field_key(&esc) && w.is_open(), "the window stays");
}

#[test]
fn windowed_the_inputs_shortcut_opens_and_closes_the_input_only() {
    assert_eq!(toggle(false, true, false, true), Toggle::ShowInput);
    assert_eq!(toggle(true, true, false, true), Toggle::HideInput);
    assert_eq!(toggle(false, true, false, false), Toggle::ShowChat, "in the side panel, as before");
}

#[test]
fn a_sent_message_gets_a_reply_fed_by_its_token() {
    let mut c = Chat::new(None);
    let id = sent(&mut c, "hi");
    let token = c.ask(id).expect("a reply");
    assert!(c.streaming());
    c.reply_event(token, Event::Text("Hello".into()), Instant::now());
    c.reply_event(token + 7, Event::Text(" stray".into()), Instant::now());
    let r = c.reply_to(id).expect("its reply");
    assert_eq!((r.prompt.as_str(), texts(r).as_str()), ("hi", "Hello"));
    c.reply_event(token, Event::Done, Instant::now());
    assert!(!c.streaming());
    assert_eq!(c.ask(99), None, "no such message");
}

#[test]
fn sending_again_or_retrying_stops_the_reply_streaming_and_its_old_events_go_nowhere() {
    let mut c = Chat::new(None);
    let first = sent(&mut c, "one");
    let old = c.ask(first).unwrap();
    let second = sent(&mut c, "two");
    let token = c.ask(second).unwrap();
    assert_eq!(c.reply_to(first).map(|r| r.state), Some(reply::State::Stopped));
    c.stop(Instant::now());
    assert_eq!(c.reply_to(second).map(|r| r.state), Some(reply::State::Stopped));
    let again = c.retry(token).expect("retried");
    c.reply_event(token, Event::Text("stale".into()), Instant::now());
    c.reply_event(old, Event::Text("stale".into()), Instant::now());
    c.reply_event(again, Event::Text("fresh".into()), Instant::now());
    let r = c.reply_to(second).unwrap();
    assert_eq!((r.state, texts(r).as_str()), (reply::State::Streaming, "fresh"), "one reply per message, started over");
}

#[test]
fn starting_over_drops_the_replies_and_stale_streams_stay_out() {
    let mut c = Chat::new(None);
    let id = sent(&mut c, "one");
    let old = c.ask(id).unwrap();
    c.start_over();
    let id = sent(&mut c, "new");
    c.reply_event(old, Event::Text("stale".into()), Instant::now());
    assert!(c.reply_to(id).is_none(), "the old stream feeds nothing");
    assert_eq!(c.replies.len(), 0);
}

#[test]
fn answering_a_question_sends_the_answer_and_gets_its_reply() {
    let mut c = Chat::new(None);
    let id = sent(&mut c, "ask");
    let token = c.ask(id).unwrap();
    let ask = Event::Ask { prompt: "Which?".into(), options: vec!["Tabs".into(), "Page".into()], multi: false };
    c.reply_event(token, ask, Instant::now());
    c.reply_event(token, Event::Done, Instant::now());
    assert_eq!(c.answer(token, 0), None, "nothing picked");
    c.pick(token, 0, 1);
    let next = c.answer(token, 0).expect("answered");
    assert_eq!(c.messages.last().map(|m| m.text.as_str()), Some("Page"));
    let r = c.replies.iter().find(|r| r.token == next).expect("its reply");
    assert!(r.answer && r.streaming());
    assert_eq!(c.answer(token, 0), None, "once");
}

pub(crate) fn tiny() -> Image {
    Image::from_rgba(1, 1, vec![0; 4])
}

/// A 2 × 1 PNG of `color`'s kind, its pixels `data`.
fn png_of(color: png::ColorType, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 2, 1);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(data).unwrap();
    out
}

/// What the clipboard holds: `text`, and an image when `png` is given.
pub(crate) fn clip(text: &str, png: Option<Vec<u8>>) -> rune::ClipboardContent {
    let content = rune::ClipboardContent::new(text);
    match png.and_then(rune::ClipboardImage::from_png) {
        Some(image) => content.with_image(image),
        None => content,
    }
}

pub(crate) fn a_png() -> Vec<u8> {
    png_of(png::ColorType::Rgba, &[1, 2, 3, 255, 4, 5, 6, 255])
}

#[test]
fn pasted_images_take_their_own_ids_and_text_is_left_to_the_field() {
    let mut c = Chat::new(None);
    assert_eq!(c.take_paste(&clip("just text", None)), vec![]);
    assert_eq!(c.take_paste(&clip("", Some(a_png()))), vec![(1, Pasted::Png(a_png()))]);
    assert_eq!(c.take_paste(&clip("", Some(a_png()))).first().map(|(id, _)| *id), Some(2));
}

#[test]
fn image_files_copied_in_finder_paste_as_images_and_other_files_as_none() {
    let files = |names: &[&str]| clip("names", Some(a_png())).with_files(names.iter().map(std::path::PathBuf::from));
    let file = |n: &str| Pasted::File(n.into());
    assert_eq!(pasted_images(&files(&["/a/one.PNG", "/a/notes.txt", "/a/two.jpeg", "/a/3.jpg"])), vec![file("/a/one.PNG"), file("/a/two.jpeg"), file("/a/3.jpg")], "the files, not Finder's icon");
    assert_eq!(pasted_images(&files(&["/a/notes.txt"])), vec![], "not the icon of a file that isn't an image");
    assert_eq!(pasted_images(&clip("", Some(a_png()))), vec![Pasted::Png(a_png())]);
    assert_eq!(pasted_images(&clip("text", None)), vec![]);
    let mut next = 0;
    assert_eq!(take_ids(&mut next, vec![file("/a"), file("/b")]), vec![(1, file("/a")), (2, file("/b"))], "one chip each");
}

#[test]
fn too_large_an_image_file_is_not_read() {
    let dir = std::env::temp_dir().join(format!("ailfred-large-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("a.png");
    std::fs::write(&png, a_png()).unwrap();
    let len = a_png().len() as u64;
    assert_eq!(image_file_png_within(&png, len - 1, 100), None, "too many bytes");
    assert_eq!(image_file_png_within(&png, len, 1), None, "too many pixels (2)");
    assert!(image_file_png_within(&png, len, 2).is_some());
    let jpeg = dir.join("a.jpg");
    std::fs::write(&jpeg, include_bytes!("../testdata/tiny.jpg")).unwrap();
    assert_eq!(image_file_png_within(&jpeg, 1 << 20, 11), None, "too many pixels (4x3)");
    assert!(image_file_png_within(&jpeg, 1 << 20, 12).is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_image_file_is_read_as_a_png() {
    let dir = std::env::temp_dir().join(format!("ailfred-paste-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("a.png");
    std::fs::write(&png, a_png()).unwrap();
    assert_eq!(image_file_png(&png), Some(a_png()), "a PNG as it is");
    let jpeg = dir.join("a.jpg");
    std::fs::write(&jpeg, include_bytes!("../testdata/tiny.jpg")).unwrap();
    let converted = image_file_png(&jpeg).expect("a JPEG converted");
    let (w, h, _) = decode_png(&converted).unwrap();
    assert_eq!((w, h), (4, 3));
    let text = dir.join("fake.png");
    std::fs::write(&text, "not an image").unwrap();
    assert_eq!(image_file_png(&text), None);
    assert_eq!(image_file_png(&dir.join("missing.png")), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn any_8_bit_png_decodes_to_rgba() {
    assert_eq!(decode_png(&png_of(png::ColorType::Rgb, &[1, 2, 3, 4, 5, 6])), Some((2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255])));
    assert_eq!(decode_png(&png_of(png::ColorType::Grayscale, &[7, 8])), Some((2, 1, vec![7, 7, 7, 255, 8, 8, 8, 255])));
    assert_eq!(decode_png(&png_of(png::ColorType::GrayscaleAlpha, &[7, 9, 8, 10])), Some((2, 1, vec![7, 7, 7, 9, 8, 8, 8, 10])));
    assert_eq!(decode_png(&a_png()).map(|(.., px)| px), Some(vec![1, 2, 3, 255, 4, 5, 6, 255]));
}

#[test]
fn a_pasted_image_lands_only_in_a_field_still_open() {
    assert!(paste_lands(Field::Input, true, false));
    assert!(paste_lands(Field::Chat, false, true));
    assert!(paste_lands(Field::Chat, true, true), "Cmd+N right after: the chat still shows");
    assert!(!paste_lands(Field::Input, false, true), "the input closed meanwhile");
    assert!(!paste_lands(Field::Chat, true, false), "the chat closed meanwhile");
}

#[test]
fn a_chips_image_opens_enlarged_and_esc_closes_it_before_the_chat() {
    let mut c = Chat::new(None);
    c.attach(1, vec![1], tiny());
    c.attach(2, vec![2], tiny());
    c.open_preview(2);
    assert!(c.preview_image().is_some());
    assert_eq!(c.escape(), Escape::Preview, "the enlarged image closes first");
    assert!(c.preview_image().is_none());
    assert_eq!(c.shot_ids(), vec![1, 2], "its capture stays attached");
    assert_eq!(c.escape(), Escape::Close, "then the chat closes");
    c.open_preview(9);
    assert!(c.preview_image().is_none(), "no such capture");
}

#[test]
fn removing_or_sending_a_capture_closes_its_enlarged_image() {
    let mut c = Chat::new(None);
    c.attach(1, vec![1], tiny());
    c.open_preview(1);
    assert!(c.remove(1));
    assert!(c.preview_image().is_none());
    assert_eq!(c.escape(), Escape::Close, "nothing enlarged left to close first");
    c.attach(2, vec![2], tiny());
    c.open_preview(2);
    c.send().expect("sent");
    assert_eq!(c.escape(), Escape::Close);
}

#[test]
fn backspace_in_an_empty_field_removes_the_last_capture() {
    let mut c = Chat::new(None);
    c.attach(1, vec![1], tiny());
    c.attach(2, vec![2], tiny());
    assert_eq!(c.backspace_empty(), Some(2));
    assert_eq!(c.backspace_empty(), Some(1));
    assert_eq!(c.backspace_empty(), None);
}

#[test]
fn the_input_shortcut_brings_back_a_started_chat() {
    assert_eq!(toggle(false, false, false, false), Toggle::ShowInput, "no chat yet");
    assert_eq!(toggle(true, false, false, false), Toggle::HideInput);
    assert_eq!(toggle(true, true, true, false), Toggle::HideInput, "an open input closes first");
    assert_eq!(toggle(false, true, false, false), Toggle::ShowChat, "the chat instead of the input");
    assert_eq!(toggle(false, true, true, false), Toggle::HideChat);
}

#[test]
fn starting_over_slides_out_and_the_next_message_starts_a_new_chat() {
    let mut c = Chat::new(None);
    c.push(msg(0, "old"));
    c.open();
    c.draft = TextEdit::new("unsent");
    c.start_over();
    assert!(!c.is_open());
    assert_eq!(c.messages.len(), 1, "still shown while it slides out");
    c.push(msg(1, "new"));
    assert_eq!(c.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), vec!["new"]);
    assert_eq!(c.draft.text, "", "the old draft went with it");
    c.push(msg(2, "more"));
    assert_eq!(c.messages.len(), 2, "then it carries on");
}

#[test]
fn thumbnails_fit_keeping_their_shape() {
    assert_eq!(thumb_size(1200, 400, (240.0, 160.0)), (240.0, 80.0), "wide");
    assert_eq!(thumb_size(400, 800, (240.0, 160.0)), (80.0, 160.0), "tall");
    assert_eq!(thumb_size(100, 50, (240.0, 160.0)), (100.0, 50.0), "never enlarged");
    assert_eq!(thumb_size(0, 0, (240.0, 160.0)), (0.0, 0.0));
}

#[test]
fn a_capture_png_decodes_back_to_its_pixels() {
    let rgba: Vec<u8> = (0..3 * 2 * 4).map(|i| i as u8).collect();
    let png = crate::export::encode_png(3, 2, 2.0, &rgba).unwrap();
    assert_eq!(decode_png(&png), Some((3, 2, rgba)));
    assert_eq!(decode_png(b"not a png"), None);
}
