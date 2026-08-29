use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandWindow {
    Inherit,
    Hidden,
}

pub(crate) fn configure_command_window(command: &mut Command, window: CommandWindow) {
    if window == CommandWindow::Hidden {
        hide_command_window(command);
    }
}

pub fn hide_command_window(command: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}
