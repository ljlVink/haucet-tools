use std::io;

pub fn read_u16(bytes: &[u8], offset: usize) -> io::Result<u16> {
    Ok(u16::from_le_bytes(read_array(bytes, offset)?))
}

pub fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(read_array(bytes, offset)?))
}

pub fn read_u64(bytes: &[u8], offset: usize) -> io::Result<u64> {
    Ok(u64::from_le_bytes(read_array(bytes, offset)?))
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> io::Result<[u8; N]> {
    let end = offset.checked_add(N).ok_or_else(|| out_of_range(offset))?;
    let slice = bytes.get(offset..end).ok_or_else(|| out_of_range(offset))?;
    let mut array = [0_u8; N];
    array.copy_from_slice(slice);
    Ok(array)
}

fn out_of_range(offset: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        format!("unexpected end of data at offset {offset:#x}"),
    )
}
