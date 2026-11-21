//! A fake model: scripted replies streamed on a timer, picked by a keyword in the message
//! ("code", "diff", "agent", "ask", "think", "error"); anything else gets a showcase of every
//! kind of reply. A real model replaces [`stream`] and keeps the [`Event`]s.

use std::time::Duration;

use crate::reply::{Event, Tool};

/// The gap between two streamed pieces of text.
const WORDS: Duration = Duration::from_millis(35);
/// Words per streamed piece.
const PER_PIECE: usize = 3;

/// What happens, and how long after the previous event.
pub type Script = Vec<(Duration, Event)>;

/// The scripted reply to `prompt`.
pub fn script(prompt: &str) -> Script {
    let p = prompt.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| p.contains(w));
    let mut s = Script::new();
    if has(&["error", "fail"]) {
        text(&mut s, "Let me look into that. The first thing to check is ");
        s.push((ms(600), Event::Fail("The model is overloaded right now. Try again in a moment.".into())));
        return s;
    }
    if has(&["ask", "question"]) {
        think(&mut s, THINK_ASK);
        text(&mut s, "Before I start, one thing to settle:");
        s.push((ms(200), ask_layout()));
    } else if has(&["diff", "patch", "change"]) {
        text(&mut s, "Here's the change: the timer now resets on every call.\n");
        s.push((ms(300), diff()));
        text(&mut s, "\nApply it and the old timer is dropped before a new one starts.");
    } else if has(&["agent", "tool", "run"]) {
        agent(&mut s);
        text(&mut s, AGENT_SUMMARY);
    } else if has(&["code"]) {
        text(&mut s, CODE_MD);
    } else if has(&["think", "reason"]) {
        think(&mut s, THINK_LONG);
        text(&mut s, "**Yes**: 17 is prime. It has no divisor between 2 and √17 ≈ 4.1 (2, 3 and 4 all leave a remainder).");
    } else {
        showcase(&mut s);
    }
    s.push((ms(100), Event::Done));
    s
}

/// The reply once the user answered a question with `answer`.
pub fn answer_script(answer: &str) -> Script {
    let mut s = Script::new();
    text(&mut s, &format!("Got it: **{answer}**. I'll go with that.\n\n- Laid out the way you picked\n- Same shortcuts as before\n\nAsk again any time."));
    s.push((ms(100), Event::Done));
    s
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// `source` in pieces of a few words, whitespace kept, so they add up to it exactly.
pub fn pieces(source: &str) -> Vec<String> {
    let words: Vec<&str> = source.split_inclusive(char::is_whitespace).collect();
    words.chunks(PER_PIECE).map(|c| c.concat()).collect()
}

fn text(s: &mut Script, source: &str) {
    for piece in pieces(source) {
        s.push((WORDS, Event::Text(piece)));
    }
}

fn think(s: &mut Script, source: &str) {
    s.push((ms(300), Event::Think(String::new())));
    for piece in pieces(source) {
        s.push((WORDS, Event::Think(piece)));
    }
}

fn agent(s: &mut Script) {
    let steps: [(Tool, &str, &str, bool, u64); 4] = [
        (Tool::Search, "\"debounce\" in src/", "src/input.rs:42: fn debounce(&mut self)\nsrc/search.rs:17: // debounce keystrokes", true, 500),
        (Tool::Read, "src/input.rs", "fn debounce(&mut self) {\n    self.timer = Some(Instant::now());\n}", true, 400),
        (Tool::Edit, "src/input.rs", "1 line changed", true, 450),
        (Tool::Run, "cargo test input", "running 12 tests\n............\ntest result: ok. 12 passed; 0 failed", true, 1100),
    ];
    for (i, (tool, target, output, ok, took)) in steps.into_iter().enumerate() {
        let id = i as u32 + 1;
        s.push((ms(150), Event::Step { id, tool, target: target.into() }));
        s.push((ms(took), Event::StepOutput { id, text: output.into() }));
        s.push((ms(120), Event::StepEnd { id, ok }));
    }
}

fn diff() -> Event {
    Event::Diff { path: "src/input.rs".into(), patch: PATCH.into() }
}

fn ask_layout() -> Event {
    Event::Ask {
        prompt: "Which layout should the settings use?".into(),
        options: vec!["Tabs at the top".into(), "One long page".into(), "Sidebar with sections".into()],
        multi: false,
    }
}

fn showcase(s: &mut Script) {
    think(s, THINK_SHOWCASE);
    text(s, SHOWCASE_MD);
    agent(s);
    text(s, "\nThe tests pass. Here's the fix:\n");
    s.push((ms(300), diff()));
    text(s, CODE_MD);
    s.push((
        ms(300),
        Event::Ask {
            prompt: "What should I do next?".into(),
            options: vec!["Add a test for it".into(), "Document the change".into(), "Open a pull request".into()],
            multi: true,
        },
    ));
}

const THINK_ASK: &str = "The settings could be laid out a few ways, and the choice changes the whole structure. Better to ask than guess.";
const THINK_LONG: &str = "Is 17 prime? I only need to try divisors up to its square root, about 4.12. 17 / 2 = 8.5, 17 / 3 ≈ 5.67, 17 / 4 = 4.25. None divide evenly, so it's prime.";
const THINK_SHOWCASE: &str = "The user wants to see what I can do. I'll explain briefly, look through the code, fix the bug, show the diff and some code, then ask how to go on.";

const SHOWCASE_MD: &str = "## What I found\n\nThe input **debounces** each keystroke, but the timer is *never reset*, so fast typing fires early. In short:\n\n1. Each key starts a timer.\n2. The old timer keeps running.\n3. The first one fires mid-word.\n\n| Before | After |\n| --- | --- |\n| fires after the *first* key | fires after the *last* key |\n| 3 searches per word | 1 search per word |\n\n> Debouncing waits for a pause; throttling fires at a steady rate.\n\nLet me check the code with `debounce` in it.\n";

const CODE_MD: &str = "\nA debounce in Rust keeps only the last call:\n\n```rust\nuse std::time::{Duration, Instant};\n\npub struct Debounce {\n    wait: Duration,\n    last: Option<Instant>,\n}\n\nimpl Debounce {\n    /// True once `wait` has passed since the last call.\n    pub fn ready(&mut self, now: Instant) -> bool {\n        let ready = self.last.is_some_and(|t| now - t >= self.wait);\n        self.last = Some(now);\n        ready\n    }\n}\n```\n\nAnd from the shell:\n\n```sh\ncargo test debounce -- --nocapture\n```\n";

const AGENT_SUMMARY: &str = "\nI found the debounce in `src/input.rs`, reset its timer on every key, and ran its tests: **12 passed**.";

const PATCH: &str = "--- a/src/input.rs\n+++ b/src/input.rs\n@@ -40,7 +40,8 @@ impl Input {\n     /// Waits for a pause in typing before searching.\n     fn debounce(&mut self) {\n-        self.timer = Some(Instant::now());\n+        // Every key restarts the wait.\n+        self.timer.replace(Instant::now());\n         self.search_later();\n     }\n";

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::reply::{Part, Reply, State};

    fn reply(prompt: &str) -> Reply {
        let mut r = Reply::new(1, prompt);
        for (_, e) in script(prompt) {
            r.apply(e, Instant::now());
        }
        r
    }

    fn kinds(r: &Reply) -> Vec<&'static str> {
        r.parts
            .iter()
            .map(|p| match p {
                Part::Thinking(_) => "think",
                Part::Text(_) => "text",
                Part::Agent(_) => "agent",
                Part::Diff(_) => "diff",
                Part::Question(_) => "ask",
                Part::Error(_) => "error",
            })
            .collect()
    }

    #[test]
    fn each_keyword_gets_its_kind_of_reply() {
        assert_eq!(kinds(&reply("show me some code")), ["text"]);
        assert_eq!(kinds(&reply("a diff please")), ["text", "diff", "text"]);
        assert_eq!(kinds(&reply("run the agent")), ["agent", "text"]);
        assert_eq!(kinds(&reply("ask me")), ["think", "text", "ask"]);
        assert_eq!(kinds(&reply("think hard")), ["think", "text"]);
        let failed = reply("make an error");
        assert_eq!((kinds(&failed), failed.state), (vec!["text", "error"], State::Failed));
    }

    #[test]
    fn anything_else_shows_every_kind_and_finishes() {
        let r = reply("hello");
        let k = kinds(&r);
        for kind in ["think", "text", "agent", "diff", "ask"] {
            assert!(k.contains(&kind), "{kind} in {k:?}");
        }
        assert_eq!(r.state, State::Done);
        assert!(matches!(r.parts.last(), Some(Part::Question(q)) if q.multi));
    }

    #[test]
    fn streamed_pieces_add_up_to_the_markdown() {
        let streamed: String = script("code").into_iter().filter_map(|(_, e)| if let Event::Text(t) = e { Some(t) } else { None }).collect();
        assert_eq!(streamed, CODE_MD);
        assert!(pieces(CODE_MD).len() > 10, "streamed in many pieces");
    }

    #[test]
    fn a_reply_takes_a_few_seconds_not_minutes() {
        for prompt in ["hello", "code", "diff", "agent", "ask", "think", "error"] {
            let total: Duration = script(prompt).iter().map(|(d, _)| *d).sum();
            assert!(total > ms(300) && total < Duration::from_secs(15), "{prompt}: {total:?}");
        }
    }

    #[test]
    fn an_answer_gets_a_reply_that_names_it() {
        let streamed: String = answer_script("Tabs").into_iter().filter_map(|(_, e)| if let Event::Text(t) = e { Some(t) } else { None }).collect();
        assert!(streamed.contains("**Tabs**"), "{streamed}");
    }
}
