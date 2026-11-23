//! A command's hand-off to the main thread: applied once the main thread takes it, never once
//! the server gave up waiting.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::super::{Answer, handed};

const WAIT: Duration = Duration::from_millis(50);

#[test]
fn a_command_the_main_thread_gets_to_after_the_wait_is_503_and_never_applied() {
    let applied = Arc::new(AtomicBool::new(false));
    let flag = applied.clone();
    let mut late = None;
    let answer = handed(WAIT, |ticket| {
        late = Some(thread::spawn(move || {
            thread::sleep(WAIT * 5);
            if ticket.take() {
                flag.store(true, Ordering::SeqCst);
                ticket.answer(Answer::accepted("t", "r"));
            }
        }));
    });
    assert_eq!(answer.status, 503);
    late.expect("posted").join().unwrap();
    assert!(!applied.load(Ordering::SeqCst), "a 503 means not applied: a retry can't send it twice");
}

#[test]
fn a_command_taken_in_time_gets_its_answer_even_when_it_comes_after_the_wait() {
    let answer = handed(WAIT, |ticket| {
        thread::spawn(move || {
            assert!(ticket.take());
            thread::sleep(WAIT * 4);
            ticket.answer(Answer::accepted("t", "r"));
        });
    });
    assert_eq!(answer, Answer::accepted("t", "r"), "it was applied: the client must hear so");
}

#[test]
fn a_command_dropped_untaken_is_503_at_once() {
    let answer = handed(Duration::from_secs(5), drop);
    assert_eq!(answer.status, 503);
}
