pub mod seed;
pub mod state; 

// Re-export the main boot function so main.rs can easily call crate::entity::awaken_entity()
pub use seed::awaken_entity;