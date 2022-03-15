pub struct Frame<const N: usize> {
    header: FrameHeader,
    data: [u8; N],
}

pub struct FrameHeader {
    id: u16,
    length: u16,
}

pub const ID_MAX: u16 = 0xF0FF;
pub const LENGTH_MAX: u16 = 0xF0FF;

fn div_round_up(a: usize, b: usize) -> usize {
    if a == 0 {
        return 1;
    }

    (a - 1) / b + 1
}

impl FrameHeader {
    fn calculate_length_field(payload_length: usize) -> Result<u16, Error> {
        // Calculate size used when encoding the given data as a Frame:
        // 6-byte header, base64 overhead, 2-byte CRC
        if payload_length >= (usize::MAX / 8) {
            return Err(Error::InvalidLength);
        }
        let packet_length = div_round_up(payload_length * 8, 6) + 2;
        if packet_length > LENGTH_MAX as usize {
            return Err(Error::InvalidLength);
        }
        Ok(packet_length as u16)
    }

    pub fn new(id: u16, payload_length: usize) -> Result<Self, Error> {
        if id > ID_MAX {
            return Err(Error::InvalidID);
        }

        let length = Self::calculate_length_field(payload_length)?;

        Ok(Self { id, length })
    }

    pub fn data_len(&self) -> usize {
        self.length as usize
    }

    pub fn total_packet_len(&self) -> usize {
        6 + self.data_len()
    }

    pub fn as_bytes(self: Self) -> [u8; 6] {
        let id_bytes: [u8; 2] = self.id.to_le_bytes();
        let len_bytes: [u8; 2] = self.length.to_le_bytes();
        [
            0xF1,
            id_bytes[0],
            id_bytes[1],
            len_bytes[0],
            len_bytes[1],
            0xFF,
        ]
    }
}

impl<const N: usize> TryFrom<&[u8]> for Frame<N> {
    type Error = Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        todo!()
    }
}

/*
 * API Option 1: (myserial should implement interface, less verbose, more efficient)
 *
 * // Tx
 * let transmitter = Transmitter::new(myserial)
 * success = transmitter.transmit(0x1337, [1,2,3,4,5])
 *
 * // Rx
 * let decoder = Decoder::new(myserial);
 *     match decoder.poll() {
 *         Ok(frame) => {do something}
 *         Err() => {}
 * }
 *
 * // RX alt:
 * let decoder = Decoder::new(myserial);
 * match decoder.read(|frame| {
 *      // frame is borrowed instead of copied
 * })
 *
 *
 * API Option2: (less coupling, more code + memory overhead)
 * // Tx
 * let frame = transmit(0x1337, [1,2,3,4,5]);
 * if myserial.space_available() > frame.raw_size()
 *     success = myserial.transmit(frame)
 *
 * // Rx
 * let decoder = Decoder::new();
 * if let some(byte) = myserial.read() {
 *     match decoder.decode(byte) {
 *         Ok(frame) => {do something}
 *         Err() => {}
 * }
 *
 */

use crate::error::Error;
