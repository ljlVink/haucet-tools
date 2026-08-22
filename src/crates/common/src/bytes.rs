use std::io;

pub fn read_u16(bytes: &[u8], offset: usize) -> io::Result<u16> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| out_of_range(offset))?;
    Ok(u16::from_le_bytes(slice.try_into().expect("2-byte slice")))
}

pub fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| out_of_range(offset))?;
    Ok(u32::from_le_bytes(slice.try_into().expect("4-byte slice")))
}

pub fn read_u64(bytes: &[u8], offset: usize) -> io::Result<u64> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| out_of_range(offset))?;
    Ok(u64::from_le_bytes(slice.try_into().expect("8-byte slice")))
}

fn out_of_range(offset: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        format!("unexpected end of data at offset {offset:#x}"),
    )
}
