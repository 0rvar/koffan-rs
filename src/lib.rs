//! Client for the [Koffan](https://github.com/PanSalut/Koffan) shopping list
//! REST API (`/api/v1`), plus a small tool layer for LLM agents.
//!
//! [`Client`] maps one method to each endpoint. [`tools::Tools`] wraps a client
//! in a handful of name-based tools (show, add, check, remove) whose replies
//! are plain text meant to be read out by a voice or chat agent.

mod client;
mod error;
pub mod matching;
pub mod model;
pub mod tools;

pub use client::Client;
pub use error::{Error, Result};
pub use model::*;
