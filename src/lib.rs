//! hub-bridge library surface. The binary in `main.rs` is a thin shell
//! over these modules so integration tests can reach the same code.

pub mod config;
pub mod hub;
pub mod route;
pub mod webhook;
