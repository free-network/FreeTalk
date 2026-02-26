//! Freenet API integration for chat board synchronization
//!
//! Handles WebSocket communication with Freenet network, manages board subscriptions,
//! and processes state updates.

pub mod board_synchronizer;
pub mod connection_manager;
pub mod constants;
pub mod error;
pub mod freenet_synchronizer;
pub mod response_handler;

pub use freenet_synchronizer::FreenetSynchronizer;
