//! View rendering modules

mod evolution;
mod side_by_side;
mod single_pane;

pub use evolution::render_evolution;
pub use side_by_side::render_side_by_side;
pub use single_pane::render_single_pane;
