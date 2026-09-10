//! Transport-neutral server protocol foundations.
//!
//! Unix listener and scheduling code is added separately. This module stays
//! available on every supported target so protocol fixtures are portable.

pub mod protocol;
