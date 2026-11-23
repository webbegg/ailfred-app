//! Which actions need a native per-action confirmation. A declared intent only adds
//! checks: an action is risky when the target is risky or the intent is.

use super::keys::KeyCombo;

/// Words that make an action risky, in English and Spanish. `place order` is a phrase.
const RISKY: &[&str] = &[
    "send", "pay", "buy", "purchase", "delete", "remove", "submit", "confirm", "transfer", "place order",
    "checkout", "enviar", "pagar", "comprar", "eliminar", "borrar", "confirmar", "transferir",
];

/// Extra words that make a text field send-like (a chat or comment composer), where
/// Return usually sends.
const SEND_LIKE_FIELD: &[&str] = &[
    "message", "messages", "reply", "comment", "chat", "post", "mensaje", "mensajes", "responder", "respuesta",
    "comentario", "comentar", "publicar",
];

/// Roles a click can press: buttons, menu items, and links (web pages often render buttons as links).
const PRESSABLE_ROLES: &[&str] = &["AXButton", "AXMenuButton", "AXMenuItem", "AXMenuBarItem", "AXLink"];
const TEXT_INPUT_ROLES: &[&str] = &["AXTextField", "AXTextArea", "AXComboBox"];

/// What the Accessibility API reports about an element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AxElement {
    pub role: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
}

/// The focused element and its window's default button, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Focus {
    pub element: AxElement,
    pub is_default_button: bool,
    pub default_button: Option<AxElement>,
}

/// The facts the classifier needs about an action.
#[derive(Debug, Clone, Copy)]
pub enum Act<'a> {
    /// A click: the element under the point first, then its ancestors.
    Click(&'a [AxElement]),
    /// A drag: the chains under the start point and under the end point.
    Drag(&'a [AxElement], &'a [AxElement]),
    /// Space, from `key`: it presses a focused button.
    Space(Option<&'a Focus>),
    /// Text from `type`: a newline presses Return and a space presses a focused button.
    Type(&'a str, Option<&'a Focus>),
    /// A combo from `key`, with what has the focus.
    Key(KeyCombo, Option<&'a Focus>),
    /// Every other action: only the intent can make it risky.
    Other,
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// The first entry of `list` found in `text` as whole words, in order, case-insensitively.
fn find_in(text: &str, list: &[&'static str]) -> Option<&'static str> {
    let words = words(text);
    list.iter().copied().find(|entry| {
        let entry: Vec<&str> = entry.split(' ').collect();
        words.windows(entry.len()).any(|window| window.iter().zip(&entry).all(|(a, b)| a == b))
    })
}

/// The first risky word or phrase in `text`, matched on whole words, case-insensitively.
pub fn risky_word(text: &str) -> Option<&'static str> {
    find_in(text, RISKY)
}

impl AxElement {
    /// The element's title, description, and value, joined for matching and messages.
    pub fn label(&self) -> String {
        [&self.title, &self.description, &self.value]
            .into_iter()
            .flatten()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn is_pressable(&self) -> bool {
        PRESSABLE_ROLES.contains(&self.role.as_str())
    }
}

fn kind(role: &str) -> &str {
    match role {
        "AXMenuItem" | "AXMenuBarItem" => "menu item",
        "AXLink" => "link",
        "AXTextField" | "AXTextArea" | "AXComboBox" => "field",
        _ => "button",
    }
}

/// A pressable element in `chain` (the element under the point or an ancestor) whose own
/// text, or the text of the elements inside it, is risky.
fn risky_click(chain: &[AxElement]) -> Option<String> {
    let pressable = chain.iter().position(AxElement::is_pressable)?;
    let label = chain[..=pressable].iter().map(AxElement::label).collect::<Vec<_>>().join(" ");
    risky_word(&label).map(|_| format!("the {} “{}”", kind(&chain[pressable].role), label.trim()))
}

fn risky_return(focus: &Focus) -> Option<String> {
    let element = &focus.element;
    let label = element.label();
    if focus.is_default_button {
        return Some(format!("Return presses the default button “{label}”"));
    }
    if element.is_pressable() && risky_word(&label).is_some() {
        return Some(format!("Return presses the {} “{label}”", kind(&element.role)));
    }
    let field = TEXT_INPUT_ROLES.contains(&element.role.as_str());
    if field && (risky_word(&label).is_some() || find_in(&label, SEND_LIKE_FIELD).is_some()) {
        return Some(format!("Return may send the field “{label}”"));
    }
    let default = focus.default_button.as_ref().map(AxElement::label)?;
    risky_word(&default).map(|_| format!("Return may press the default button “{default}”"))
}

/// Space presses the focused button; in a field it only types a space.
fn risky_space(focus: &Focus) -> Option<String> {
    let element = &focus.element;
    let label = element.label();
    if focus.is_default_button {
        return Some(format!("Space presses the default button “{label}”"));
    }
    if element.is_pressable() && risky_word(&label).is_some() {
        return Some(format!("Space presses the {} “{label}”", kind(&element.role)));
    }
    None
}

/// Roles that tell nothing about what is there: an app without an Accessibility tree (Chrome or
/// Electron often) answers with its window, its app, or nothing at all.
const UNREADABLE_ROLES: &[&str] = &["", "AXUnknown", "AXWindow", "AXApplication"];

fn unreadable(chain: &[AxElement]) -> bool {
    chain.iter().all(|e| UNREADABLE_ROLES.contains(&e.role.as_str()))
}

/// A click's chain that tells nothing could be a Send or Buy button: it asks.
fn checked_click(chain: &[AxElement]) -> Option<String> {
    if unreadable(chain) {
        return Some("the missing Accessibility details of what is under the point (it may be a Send or Buy button)".into());
    }
    risky_click(chain)
}

/// The focus, unless it tells nothing.
fn readable(focus: Option<&Focus>) -> Option<&Focus> {
    focus.filter(|f| !unreadable(std::slice::from_ref(&f.element)))
}

fn unread_focus(what: &str) -> String {
    format!("the missing Accessibility details of what has the focus ({what})")
}

fn checked_return(focus: Option<&Focus>) -> Option<String> {
    readable(focus).map_or_else(|| Some(unread_focus("Return may send it")), risky_return)
}

/// Shortcuts that delete, quit, log out, lock or send, whatever has the focus. ⌘⌫ in a text
/// field only deletes to the line's start.
fn risky_shortcut(combo: KeyCombo, focus: Option<&Focus>) -> Option<String> {
    let m = combo.modifiers;
    let in_field = readable(focus).is_some_and(|f| TEXT_INPUT_ROLES.contains(&f.element.role.as_str()));
    let does = match combo.key {
        "delete" | "forwarddelete" if m.cmd && !in_field => "deletes what is selected (files, messages)",
        "q" if m.cmd && m.shift => "logs out",
        "q" if m.cmd && m.ctrl => "locks the screen",
        "q" if m.cmd => "quits the app",
        "d" if m.cmd && m.shift => "sends the message in Mail",
        "return" if m.cmd || m.ctrl || m.alt => "often sends or submits",
        _ => return None,
    };
    Some(format!("the shortcut {}, which {does}", combo.label()))
}

fn risky_typing(text: &str, focus: &Focus) -> Option<String> {
    let newline = text.contains('\n').then(|| risky_return(focus)).flatten();
    newline.or_else(|| text.contains(' ').then(|| risky_space(focus)).flatten())
}

/// Why `act` needs a confirmation, or `None` when it does not.
pub fn assess(act: Act<'_>, intent: Option<&str>) -> Option<String> {
    let target = match act {
        Act::Click(chain) => checked_click(chain),
        Act::Drag(from, to) => match (checked_click(from), checked_click(to)) {
            (Some(from), Some(to)) if from != to => Some(format!("{from} and {to}")),
            (from, to) => from.or(to),
        },
        Act::Space(focus) => readable(focus).map_or_else(|| Some(unread_focus("Space may press it")), risky_space),
        Act::Type(text, focus) => match readable(focus) {
            Some(focus) => risky_typing(text, focus),
            None => text.contains('\n').then(|| unread_focus("a typed Return may send it")),
        },
        Act::Key(combo, focus) => risky_shortcut(combo, focus).or_else(|| {
            if combo.is_return() {
                checked_return(focus)
            } else if combo.is_space() {
                assess(Act::Space(focus), None)
            } else {
                None
            }
        }),
        Act::Other => None,
    };
    let intent = intent.filter(|i| risky_word(i).is_some()).map(|i| format!("the declared intent “{}”", i.trim()));
    match (target, intent) {
        (Some(target), Some(intent)) => Some(format!("{target}; {intent}")),
        (target, intent) => target.or(intent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(role: &str, title: &str) -> AxElement {
        AxElement { role: role.into(), title: Some(title.into()), ..AxElement::default() }
    }

    fn focus(element: AxElement) -> Focus {
        Focus { element, ..Focus::default() }
    }

    #[test]
    fn english_and_spanish_words_match_case_insensitively() {
        assert_eq!(risky_word("SEND now"), Some("send"));
        assert_eq!(risky_word("Pagar ahora"), Some("pagar"));
        assert_eq!(risky_word("Eliminar conversación"), Some("eliminar"));
        assert_eq!(risky_word("Proceed to Checkout"), Some("checkout"));
    }

    #[test]
    fn words_match_whole_words_and_the_phrase_in_order() {
        assert_eq!(risky_word("Place Order"), Some("place order"));
        assert_eq!(risky_word("order a place"), None);
        assert_eq!(risky_word("Sender"), None);
        assert_eq!(risky_word("Paypal"), None);
        assert_eq!(risky_word("send-button"), Some("send"));
        assert_eq!(risky_word(""), None);
    }

    #[test]
    fn clicking_a_risky_button_or_menu_item_is_risky() {
        assert!(assess(Act::Click(&[element("AXButton", "Send")]), None).is_some());
        assert!(assess(Act::Click(&[element("AXMenuItem", "Delete Message")]), None).is_some());
        let described = AxElement { role: "AXButton".into(), description: Some("Comprar".into()), ..AxElement::default() };
        assert!(assess(Act::Click(&[described]), None).is_some());
    }

    #[test]
    fn text_inside_a_risky_button_counts() {
        let label = AxElement { role: "AXStaticText".into(), value: Some("Buy now".into()), ..AxElement::default() };
        let chain = [label, AxElement { role: "AXButton".into(), ..AxElement::default() }];
        assert!(assess(Act::Click(&chain), None).is_some());
    }

    #[test]
    fn harmless_clicks_are_not_risky() {
        assert_eq!(assess(Act::Click(&[element("AXButton", "Open")]), None), None);
        // A risky word outside a button (plain text, a text field) is not a press.
        assert_eq!(assess(Act::Click(&[element("AXStaticText", "Send")]), None), None);
        assert_eq!(assess(Act::Click(&[element("AXTextField", "Delete")]), None), None);
        // Readable, but nothing pressable in it: a page's plain area.
        assert_eq!(assess(Act::Click(&[element("AXGroup", ""), element("AXWebArea", "Inbox")]), None), None);
    }

    #[test]
    fn a_click_where_the_app_tells_nothing_is_risky() {
        // Chrome or Electron without an Accessibility tree: nothing, or only the window.
        let unreadable = assess(Act::Click(&[]), None).expect("nothing under the point");
        assert!(unreadable.contains("Accessibility"), "{unreadable}");
        assert!(assess(Act::Click(&[element("AXWindow", "Slack"), element("AXApplication", "Slack")]), None).is_some());
        assert!(assess(Act::Click(&[element("AXUnknown", "")]), None).is_some());
    }

    #[test]
    fn return_on_a_default_button_is_risky() {
        let default = Focus { element: element("AXButton", "OK"), is_default_button: true, default_button: None };
        assert!(assess(Act::Key(key("return"), Some(&default)), None).is_some());
    }

    #[test]
    fn return_on_a_focused_risky_button_is_risky() {
        assert!(assess(Act::Key(key("return"), Some(&focus(element("AXButton", "Transfer")))), None).is_some());
    }

    #[test]
    fn return_in_a_send_like_field_is_risky() {
        let composer = AxElement { role: "AXTextArea".into(), description: Some("Message #general".into()), ..AxElement::default() };
        assert!(assess(Act::Key(key("return"), Some(&focus(composer))), None).is_some());
        let spanish = AxElement { role: "AXTextField".into(), title: Some("Escribe un mensaje".into()), ..AxElement::default() };
        assert!(assess(Act::Key(key("return"), Some(&focus(spanish))), None).is_some());
    }

    #[test]
    fn return_with_a_risky_default_button_in_the_window_is_risky() {
        let field = Focus {
            element: element("AXTextField", "Amount"),
            is_default_button: false,
            default_button: Some(element("AXButton", "Pay")),
        };
        assert!(assess(Act::Key(key("return"), Some(&field)), None).is_some());
    }

    #[test]
    fn return_in_an_ordinary_field_is_not_risky() {
        let search = Focus {
            element: element("AXTextField", "Search"),
            is_default_button: false,
            default_button: Some(element("AXButton", "Done")),
        };
        assert_eq!(assess(Act::Key(key("return"), Some(&search)), None), None);
    }

    #[test]
    fn return_space_or_a_typed_newline_where_the_focus_tells_nothing_is_risky() {
        let window = focus(element("AXWindow", "Slack"));
        for focus in [None, Some(&window)] {
            assert!(assess(Act::Key(key("return"), focus), None).is_some(), "{focus:?}");
            assert!(assess(Act::Space(focus), None).is_some(), "{focus:?}");
            assert!(assess(Act::Type("hi\n", focus), None).is_some(), "{focus:?}");
            // Without a newline, typing presses nothing.
            assert_eq!(assess(Act::Type("a b", focus), None), None, "{focus:?}");
        }
    }

    fn key(combo: &str) -> crate::computer::keys::KeyCombo {
        crate::computer::keys::parse_combo(combo).unwrap()
    }

    #[test]
    fn shortcuts_that_delete_quit_log_out_lock_or_send_are_risky() {
        let list = focus(element("AXRow", "Report.pdf"));
        for combo in ["cmd+backspace", "cmd+shift+delete", "alt+cmd+backspace", "cmd+forwarddelete", "cmd+q", "shift+cmd+q", "alt+shift+cmd+q", "ctrl+cmd+q", "cmd+shift+d", "cmd+return", "ctrl+enter"] {
            let reason = assess(Act::Key(key(combo), Some(&list)), None);
            assert!(reason.as_deref().is_some_and(|r| r.contains(&key(combo).label())), "{combo}: {reason:?}");
        }
    }

    #[test]
    fn ordinary_shortcuts_and_deleting_text_in_a_field_are_not() {
        let list = focus(element("AXRow", "Report.pdf"));
        for combo in ["cmd+c", "cmd+v", "cmd+t", "cmd+w", "tab", "escape", "down", "backspace", "cmd+shift+t"] {
            assert_eq!(assess(Act::Key(key(combo), Some(&list)), None), None, "{combo}");
        }
        // ⌘⌫ in a field deletes to the line's start.
        let field = focus(element("AXTextField", "Search"));
        assert_eq!(assess(Act::Key(key("cmd+backspace"), Some(&field)), None), None);
        // Plain Return and Space keep their own checks.
        assert_eq!(assess(Act::Key(key("return"), Some(&field)), None), None);
        assert!(assess(Act::Key(key("return"), None), None).is_some());
        assert!(assess(Act::Key(key("space"), Some(&focus(element("AXButton", "Send")))), None).is_some());
    }

    #[test]
    fn a_drag_from_or_onto_a_risky_button_is_risky() {
        let send = [element("AXButton", "Send")];
        let open = [element("AXButton", "Open")];
        assert!(assess(Act::Drag(&send, &send), None).is_some());
        assert!(assess(Act::Drag(&send, &open), None).is_some());
        assert!(assess(Act::Drag(&open, &send), None).is_some());
        assert_eq!(assess(Act::Drag(&open, &open), None), None);
        assert!(assess(Act::Drag(&open, &[]), None).is_some(), "an end that tells nothing");
    }

    #[test]
    fn space_on_a_focused_risky_or_default_button_is_risky() {
        assert!(assess(Act::Space(Some(&focus(element("AXButton", "Send")))), None).is_some());
        let default = Focus { element: element("AXButton", "OK"), is_default_button: true, default_button: None };
        assert!(assess(Act::Space(Some(&default)), None).is_some());
    }

    #[test]
    fn space_elsewhere_is_not_risky() {
        assert_eq!(assess(Act::Space(Some(&focus(element("AXButton", "Open")))), None), None);
        // Space types in a field, even a send-like one or one with a risky default button.
        let composer = Focus {
            element: element("AXTextArea", "Message"),
            is_default_button: false,
            default_button: Some(element("AXButton", "Send")),
        };
        assert_eq!(assess(Act::Space(Some(&composer)), None), None);
    }

    #[test]
    fn typed_text_checks_its_spaces_and_newlines() {
        let send = focus(element("AXButton", "Send"));
        assert!(assess(Act::Type(" ", Some(&send)), None).is_some());
        assert_eq!(assess(Act::Type("ok", Some(&send)), None), None);
        let composer = focus(element("AXTextArea", "Message"));
        assert_eq!(assess(Act::Type("hello there", Some(&composer)), None), None);
        assert!(assess(Act::Type("hello\n", Some(&composer)), None).is_some());
    }

    #[test]
    fn a_risky_intent_makes_any_action_risky() {
        assert!(assess(Act::Other, Some("borrar el archivo")).is_some());
        assert!(assess(Act::Click(&[element("AXButton", "Open")]), Some("purchase the plan")).is_some());
    }

    #[test]
    fn a_harmless_intent_never_removes_a_check() {
        assert_eq!(assess(Act::Other, Some("open the menu")), None);
        assert!(assess(Act::Click(&[element("AXButton", "Send")]), Some("just looking")).is_some());
    }
}
