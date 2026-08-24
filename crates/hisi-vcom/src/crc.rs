pub fn crc16_hqx(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data.iter().chain(&[0u8, 0u8]) {
        let mut feedback: u16 = crc & 0xFF00;
        for _ in 0..8 {
            feedback = if feedback & 0x8000 != 0 {
                (feedback << 1) ^ 0x1021
            } else {
                feedback << 1
            };
        }
        crc = (((crc & 0xFF) << 8) | b as u16) ^ feedback;
    }
    crc
}

pub fn crc16_hqx_be(data: &[u8]) -> [u8; 2] {
    crc16_hqx(data).to_be_bytes()
}
