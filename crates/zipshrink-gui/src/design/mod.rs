//! ZipShrink's design system: tokens (colour, type, spacing, radii,
//! elevation, motion), the appearance, icons and the component library.

pub mod chrome;
pub mod fonts;
pub mod icons;
pub mod motion;
pub mod theme;
pub mod tokens;
pub mod widgets;

pub use icons::Icon;
pub use theme::pal;
pub use tokens::{space, typ};
pub use widgets::*;
