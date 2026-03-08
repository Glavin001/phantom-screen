pub mod auth;
pub mod config;
pub mod input;

// Re-export commonly used types for integration tests
pub use config::{Config, EncoderType};
pub use input::{InputEvent, estimate_event_length, parse_input_event};
