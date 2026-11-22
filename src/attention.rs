//! The mark on the menu-bar icon: a reply ended while the chat was off screen, and the chat
//! hasn't been shown since.

#[derive(Debug, Default)]
pub struct Attention {
    marked: bool,
}

impl Attention {
    /// A reply ended (done or failed). True when the mark has to come on.
    pub fn reply_ended(&mut self, chat_on_screen: bool) -> bool {
        if chat_on_screen || self.marked {
            return false;
        }
        self.marked = true;
        true
    }

    /// The chat came on screen. True when the mark has to go.
    pub fn chat_shown(&mut self) -> bool {
        std::mem::replace(&mut self.marked, false)
    }

    #[cfg(test)]
    pub fn marked(&self) -> bool {
        self.marked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_that_ends_while_the_chat_is_hidden_marks_the_icon_once() {
        let mut a = Attention::default();
        assert!(a.reply_ended(false));
        assert!(a.marked());
        assert!(!a.reply_ended(false), "already marked: nothing to change");
        assert!(a.marked());
    }

    #[test]
    fn a_reply_that_ends_with_the_chat_on_screen_marks_nothing() {
        let mut a = Attention::default();
        assert!(!a.reply_ended(true));
        assert!(!a.marked());
    }

    #[test]
    fn showing_the_chat_takes_the_mark_away() {
        let mut a = Attention::default();
        assert!(!a.chat_shown(), "no mark: nothing to change");
        a.reply_ended(false);
        assert!(a.chat_shown());
        assert!(!a.marked());
        assert!(!a.chat_shown());
    }

    #[test]
    fn a_mark_stays_when_the_next_reply_ends_on_screen() {
        // Only showing the chat takes it away: a reply seen doesn't say the earlier one was.
        let mut a = Attention::default();
        a.reply_ended(false);
        assert!(!a.reply_ended(true));
        assert!(a.marked());
    }
}
