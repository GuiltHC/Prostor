//! The pages of the main window.

pub mod analyzer;
pub mod cleanup;
pub mod dashboard;
pub mod journal;
pub mod programs;
pub mod settings;
pub mod startup;

/// Asks the main window to do something on another page:
/// a page name ("settings"), "analyze:<path>", "analyze-tab:<tab>:<path>", "programs:unused",
/// "programs:updates", "programs:games", "reload:programs", "reload:startup", "reload:cleanup".
pub struct Navigate(pub String);

impl Navigate {
    pub fn to(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}
