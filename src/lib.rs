//! Client for the [Koffan](https://github.com/PanSalut/Koffan) shopping list
//! REST API (`/api/v1`), plus a small tool layer for LLM agents.
//!
//! [`Client`] maps one method to each endpoint.

mod client;
mod error;
pub mod matching;
pub mod model;

pub use client::Client;
pub use error::{Error, Result};
pub use model::*;
