use std::process::Command;

fn extractor() -> Command {
    Command::new(env!("CARGO_BIN_EXE_extract-erofs"))
}
