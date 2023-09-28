//! ad :: the adaptable editor
pub mod buffer;
pub mod editor;
pub mod key;
pub mod term;

pub const VERSION: &str = "1.0.0";
pub const UNNAMED_BUFFER: &str = "[No Name]";
pub const TAB_STOP: usize = 4;
pub const STATUS_TIMEOUT: u64 = 5;
pub const MAX_NAME_LEN: usize = 20;

/// Helper for panicing the program but first ensuring that the screen is cleared
#[macro_export]
macro_rules! die {
    ($template:expr $(, $arg:expr)*) => {{
        $crate::term::clear_screen(&mut ::std::io::stdout());
        panic!($template $(, $arg)*)
    }};

}
