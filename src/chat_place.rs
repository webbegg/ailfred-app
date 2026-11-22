//! Where the chat lives, the side panel or its own window, and what waits for it while it moves
//! between them: the host's bookkeeping, apart from the windows so it can be tested.
//!
//! A move is two hops on the UI thread (out of one window's state, into the other's), so
//! anything for the chat in between (a reply's next piece, a decoded capture, a message sent
//! from the input) is held, then handed over in order once it arrives, or back where it was if
//! it can't.

/// Where the chat is, or is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    Side,
    Window,
}

/// Where something for the chat goes now.
#[derive(Debug, PartialEq, Eq)]
pub enum Route<U> {
    Side(U),
    Window(U),
    /// It's moving: held until it arrives.
    Held,
}

pub struct ChatPlace<U> {
    to: Where,
    held: Option<Vec<U>>,
}

impl<U> Default for ChatPlace<U> {
    fn default() -> Self {
        ChatPlace { to: Where::Side, held: None }
    }
}

impl<U> ChatPlace<U> {
    /// In its window, or on its way there.
    pub fn windowed(&self) -> bool {
        self.to == Where::Window
    }

    pub fn moving(&self) -> bool {
        self.held.is_some()
    }

    /// Where `u` goes: to the chat where it is, or held while it moves.
    pub fn route(&mut self, u: U) -> Route<U> {
        match (&mut self.held, self.to) {
            (Some(held), _) => {
                held.push(u);
                Route::Held
            }
            (None, Where::Side) => Route::Side(u),
            (None, Where::Window) => Route::Window(u),
        }
    }

    /// Starts a move to `to`: false (nothing starts) if it's there or already moving.
    pub fn start(&mut self, to: Where) -> bool {
        if self.moving() || self.to == to {
            return false;
        }
        (self.to, self.held) = (to, Some(Vec::new()));
        true
    }

    /// It arrived: what waited for it, in order.
    pub fn arrived(&mut self) -> Vec<U> {
        self.held.take().unwrap_or_default()
    }

    /// It couldn't get there: it stays where it was, with what waited for it, in order.
    pub fn stayed(&mut self) -> Vec<U> {
        if self.moving() {
            self.to = match self.to {
                Where::Side => Where::Window,
                Where::Window => Where::Side,
            };
        }
        self.held.take().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_rest_everything_goes_straight_to_the_chat_where_it_is() {
        let mut p = ChatPlace::default();
        assert_eq!(p.route(1), Route::Side(1));
        assert!(p.start(Where::Window));
        p.arrived();
        assert!(p.windowed() && !p.moving());
        assert_eq!(p.route(2), Route::Window(2));
    }

    #[test]
    fn while_it_moves_what_comes_waits_and_arrives_once_in_order() {
        let mut p = ChatPlace::default();
        assert!(p.start(Where::Window));
        assert!(p.windowed() && p.moving(), "on its way to its window: sends go there");
        assert_eq!(p.route(1), Route::Held);
        assert_eq!(p.route(2), Route::Held);
        assert!(!p.start(Where::Side) && !p.start(Where::Window), "one move at a time");
        assert_eq!(p.arrived(), vec![1, 2]);
        assert!(p.arrived().is_empty(), "handed over once");
        assert!(p.start(Where::Side));
        assert!(!p.windowed());
        assert_eq!(p.route(3), Route::Held);
        assert_eq!(p.arrived(), vec![3]);
        assert_eq!(p.route(4), Route::Side(4));
    }

    #[test]
    fn a_move_to_where_it_is_doesnt_start() {
        let mut p = ChatPlace::<u8>::default();
        assert!(!p.start(Where::Side));
        assert!(!p.moving());
    }

    #[test]
    fn a_move_that_cant_finish_leaves_it_where_it_was_with_what_waited() {
        let mut p = ChatPlace::default();
        assert!(p.start(Where::Window));
        assert_eq!(p.route(1), Route::Held);
        assert_eq!(p.stayed(), vec![1]);
        assert!(!p.windowed() && !p.moving());
        assert_eq!(p.route(2), Route::Side(2), "not held forever");
    }
}
