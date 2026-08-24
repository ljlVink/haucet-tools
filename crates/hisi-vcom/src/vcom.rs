use std::thread;
use std::time::Duration;

use crate::crc::crc16_hqx_be;
use crate::error::Error;
use crate::transport::Transport;

pub const START_FRAME: [u8; 14] = [
    0xFE, 0x00, 0xFF, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x02, 0x01, 0x1D, 0x0F,
];

pub const MAX_DATA_LEN: usize = 0x400;

const ACK_TIMEOUT: Duration = Duration::from_secs(10);

pub fn head_command(address: u32, length: u32) -> Vec<u8> {
    let mut cmd = Vec::with_capacity(12);
    cmd.extend_from_slice(&[0xFE, 0x00, 0xFF, 0x01]);
    cmd.extend_from_slice(&length.to_be_bytes());
    cmd.extend_from_slice(&address.to_be_bytes());
    cmd.extend_from_slice(&crc16_hqx_be(&cmd));
    cmd
}

pub fn data_command(seq: u8, chunk: &[u8]) -> Vec<u8> {
    let mut cmd = Vec::with_capacity(3 + chunk.len() + 2);
    cmd.push(0xDA);
    cmd.push(seq);
    cmd.push(!seq);
    cmd.extend_from_slice(chunk);
    cmd.extend_from_slice(&crc16_hqx_be(&cmd));
    cmd
}

pub fn tail_command(seq: u8) -> Vec<u8> {
    let mut cmd = vec![0xED, seq, !seq];
    cmd.extend_from_slice(&crc16_hqx_be(&cmd));
    cmd
}

pub fn write_and_verify(
    transport: &mut dyn Transport,
    command: &[u8],
    log: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    transport.discard_input();
    transport.write_all(command, ACK_TIMEOUT)?;
    let mut ack = [0u8; 1];
    transport.read_raw_timeout(&mut ack, ACK_TIMEOUT)?;
    log(&format!("  ACK byte: 0x{:02X}", ack[0]));
    if ack[0] != 0xAA {
        return Err(Error::BadAck {
            expected: 0xAA,
            actual: ack[0],
        });
    }
    Ok(())
}

pub fn send_start_frame(
    transport: &mut dyn Transport,
    log: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    transport.write_all(&START_FRAME, ACK_TIMEOUT)?;
    log("Start frame sent");
    thread::sleep(Duration::from_millis(50));
    Ok(())
}

pub fn upload(
    transport: &mut dyn Transport,
    data: &[u8],
    address: u32,
    log: &mut dyn FnMut(&str),
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), Error> {
    log(&format!(
        "Uploading {} bytes to 0x{:08X}",
        data.len(),
        address
    ));
    write_and_verify(transport, &head_command(address, data.len() as u32), log)?;

    let mut seq: u64 = 0;
    let mut sent: u64 = 0;
    for chunk in data.chunks(MAX_DATA_LEN) {
        seq += 1;
        write_and_verify(transport, &data_command((seq & 0xFF) as u8, chunk), log)?;
        sent += chunk.len() as u64;
        progress(sent, data.len() as u64);
    }

    write_and_verify(transport, &tail_command(((seq + 1) & 0xFF) as u8), log)?;
    thread::sleep(Duration::from_millis(500));
    Ok(())
}
