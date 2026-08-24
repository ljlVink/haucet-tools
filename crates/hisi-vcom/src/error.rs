use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("nusb error: {0}")]
    Nusb(#[from] nusb::Error),
    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("USB transfer error: {0}")]
    Transfer(#[from] nusb::transfer::TransferError),
    #[error("no matching USB device found ({0})")]
    NoDevice(String),
    #[error("no usable VCOM interface (bulk IN + bulk OUT) found on the device")]
    NoVcomInterface,
    #[error("timeout waiting for device response")]
    Timeout,
    #[error("invalid ACK: expected 0x{expected:02X}, got 0x{actual:02X}")]
    BadAck { expected: u8, actual: u8 },
}
