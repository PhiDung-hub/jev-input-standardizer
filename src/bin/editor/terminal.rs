use std::io::{self, IsTerminal, Read, Write};

use super::paint::Paint;

#[cfg(unix)]
use rustix::termios::{
    LocalModes, OptionalActions, OutputModes, SpecialCodeIndex, Termios, tcgetattr, tcsetattr,
};

const ENTER_ALTERNATE_SCREEN: &str = "\x1b[?1049h\x1b[2J\x1b[H";
const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";

/// A temporary screen with newline processing on, restoring the host TUI's exact
/// terminal mode when dropped.
pub(super) struct AlternateScreen {
    active: bool,
    #[cfg(unix)]
    original_terminal_mode: Option<Termios>,
}

impl AlternateScreen {
    pub(super) fn enter() -> io::Result<Self> {
        let active = io::stdout().is_terminal();
        let mut screen = Self {
            active,
            #[cfg(unix)]
            original_terminal_mode: if active && io::stdin().is_terminal() {
                Some(display_mode()?)
            } else {
                None
            },
        };
        if active {
            let mut output = io::stdout().lock();
            let entered = write!(output, "{ENTER_ALTERNATE_SCREEN}").and_then(|()| output.flush());
            if let Err(error) = entered {
                screen.restore_terminal_mode();
                return Err(error);
            }
        }
        Ok(screen)
    }

    #[cfg(unix)]
    fn restore_terminal_mode(&mut self) {
        if let Some(original) = self.original_terminal_mode.take() {
            let _ = tcsetattr(io::stdin(), OptionalActions::Now, &original);
        }
    }

    #[cfg(not(unix))]
    fn restore_terminal_mode(&mut self) {}
}

impl Drop for AlternateScreen {
    fn drop(&mut self) {
        if self.active {
            let mut output = io::stdout().lock();
            let _ = write!(output, "{LEAVE_ALTERNATE_SCREEN}");
            let _ = output.flush();
        }
        self.restore_terminal_mode();
    }
}

/// Turns on output newline processing (host TUIs often disable it) and returns the
/// mode to restore.
#[cfg(unix)]
fn display_mode() -> io::Result<Termios> {
    let stdin = io::stdin();
    let original = tcgetattr(&stdin).map_err(io::Error::from)?;
    let mut display = original.clone();
    display
        .output_modes
        .insert(OutputModes::OPOST | OutputModes::ONLCR);
    tcsetattr(&stdin, OptionalActions::Now, &display).map_err(io::Error::from)?;
    Ok(original)
}

/// Keeps the terminal in single-key mode (no Enter, no echo) until dropped.
pub(super) struct Keys {
    #[cfg(unix)]
    original: Termios,
}

impl Keys {
    #[cfg(unix)]
    pub(super) fn open() -> io::Result<Self> {
        let stdin = io::stdin();
        let original = tcgetattr(&stdin).map_err(io::Error::from)?;
        let mut single_key = original.clone();
        single_key
            .local_modes
            .remove(LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG);
        single_key.special_codes[SpecialCodeIndex::VMIN] = 1;
        single_key.special_codes[SpecialCodeIndex::VTIME] = 0;
        tcsetattr(&stdin, OptionalActions::Now, &single_key).map_err(io::Error::from)?;
        Ok(Self { original })
    }

    #[cfg(not(unix))]
    pub(super) fn open() -> io::Result<Self> {
        Ok(Self {})
    }
}

/// Shows why Jev failed and waits for one key: `r` retries, anything else gives up.
pub(super) fn ask_retry(message: &str) -> io::Result<bool> {
    let paint = Paint::detect(io::stdout().is_terminal());
    let mut output = io::stdout().lock();
    write!(
        output,
        "{}\n{} ",
        paint.red(&format!("Jev failed: {message}")),
        paint.dim("r retry · any other key keeps the draft")
    )?;
    output.flush()?;
    drop(output);
    let _keys = Keys::open()?;
    let key = read_key()?;
    println!();
    Ok(matches!(key, b'r' | b'R'))
}

pub(super) fn read_key() -> io::Result<u8> {
    let mut byte = [0_u8; 1];
    io::stdin().lock().read_exact(&mut byte)?;
    Ok(byte[0])
}

#[cfg(unix)]
impl Drop for Keys {
    fn drop(&mut self) {
        let _ = tcsetattr(io::stdin(), OptionalActions::Now, &self.original);
    }
}
