//! Computer control (roadmap S6): the model sees the screen and works the Mac's apps through
//! tools Ailfred serves, within guards it can't change. Ported from Gentle Dot's helper
//! (`apps/desktop/src-tauri/src/computer`), macOS only.
//!
//! So far only the rules, each a pure module with its tests: the rate limit, screenshot
//! geometry, key combos, the apps never targeted, which actions ask first, and the session
//! the user grants. The tools that use them come in the next slices; until then
//! nothing calls them (remove this allow once they do).
#![allow(dead_code)]

pub mod blocklist;
pub mod coords;
pub mod keys;
pub mod rate;
pub mod risk;
pub mod session;
