pub mod seed;
// pub mod state; // We will uncomment this in Phase 2

// Re-export the main boot function so main.rs can easily call crate::entity::awaken_entity()
pub use seed::awaken_entity;