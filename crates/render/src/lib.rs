//! depweave-render: software rendering (tiny-skia) and hit-testing for the
//! dependency graph. Knows nothing about pacman; it draws generic scenes.

pub mod draw;
pub mod lines;
pub mod palette;
pub mod scene;
pub mod svg;
pub mod text;
pub mod view;

pub use draw::{Highlight, Renderer, Style};
pub use scene::{Edge, Node, Scene};
pub use tiny_skia;
pub use view::View;
