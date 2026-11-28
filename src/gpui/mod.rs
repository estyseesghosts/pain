//! GPUI-specific terminal view and image texture cache. This is application
//! composition code for now; Task 10 moves it into `pain-terminal-gpui`.

#[cfg(feature = "shell")]
pub mod images;
#[cfg(feature = "shell")]
pub mod render;
