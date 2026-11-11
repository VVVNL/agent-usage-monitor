//! Provider adapters. Each adapter owns retrieval, parsing, authentication
//! detection and provider-specific errors, and returns normalised
//! [`aum_core::ProviderSnapshot`]s.

pub mod claude;
pub mod codex;
pub mod runner;
