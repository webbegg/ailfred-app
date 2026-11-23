//! Ailfred's core: the conversation model ([`reply`]), the engines that write the replies (pi
//! in RPC mode, [`pi`]; the fake one, [`mock_model`]) and what they need to run. No UI: the
//! app (and any later client) draws this state and sends the user's actions back.

pub mod mock_model;
pub mod pi;
pub mod reply;
pub mod spawn;
pub mod turn;
