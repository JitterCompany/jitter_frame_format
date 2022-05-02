use crate::error::Error;
use crc::{Crc, CRC_16_USB};

#[derive(Debug)]
pub struct Frame<const N: usize> {
    header: FrameHeader,
    data: [u8; N],
}

#[derive(Debug)]
pub(crate) struct FrameHeader {
    id: u16,
    length: u16, // NB: length of base64-data
}

pub const START_OF_FRAME: u8 = 0xF1;
pub const END_OF_HEADER: u8 = 0xFF;
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
        // 6-byte header, 2-byte CRC, base64 overhead

        // No payload: there won't be any CRC or base64 overhead
        if payload_length == 0 {
            return Ok(0);
        }

        // prevent int overflow in calculation
        if payload_length >= ((usize::MAX / 8) - 2) {
            return Err(Error::InvalidLength);
        }
        let packet_length = div_round_up((payload_length + 2) * 8, 6);
        if packet_length > LENGTH_MAX as usize {
            return Err(Error::InvalidLength);
        }
        Ok(packet_length as u16)
    }

    /// Create a FrameHeader, calculating length field based on payload_length
    pub fn new(id: u16, payload_length: usize) -> Result<Self, Error> {
        let length = Self::calculate_length_field(payload_length)?;

        Self::from_raw(id, length)
    }

    /// Create a FrameHeader from raw field values
    fn from_raw(id: u16, length: u16) -> Result<Self, Error> {
        if id > ID_MAX {
            return Err(Error::InvalidID);
        }
        if length > LENGTH_MAX {
            return Err(Error::InvalidLength);
        }

        Ok(Self { id, length })
    }

    pub fn data_len(&self) -> usize {
        self.length as usize
    }

    pub fn total_packet_len(&self) -> usize {
        6 + self.data_len()
    }

    pub fn payload_len(&self) -> usize {
        // base64 to binary: 6 bits per character
        let binary_len = self.data_len() * 6 / 8;

        if binary_len >= 2 {
            // excluding 2-byte CRC
            binary_len - 2
        } else {
            // No payload data
            0
        }
    }

    pub fn as_bytes(self: Self) -> [u8; 6] {
        let id_bytes: [u8; 2] = self.id.to_le_bytes();
        let len_bytes: [u8; 2] = self.length.to_le_bytes();
        [
            START_OF_FRAME,
            id_bytes[0],
            id_bytes[1],
            len_bytes[0],
            len_bytes[1],
            END_OF_HEADER,
        ]
    }
}

impl TryFrom<[u8; 6]> for FrameHeader {
    type Error = Error;

    fn try_from(slice: [u8; 6]) -> Result<Self, Self::Error> {
        Self::try_from(&slice[0..6])
    }
}

impl TryFrom<&[u8]> for FrameHeader {
    type Error = Error;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        if slice.len() != 6 {
            return Err(Error::TooFewBytes);
        }
        if slice.len() > 6 {
            return Err(Error::TooManyBytes);
        }

        // Parse start-of-frame marker
        if slice[0] != START_OF_FRAME {
            return Err(Error::InvalidHeader);
        }

        // Parse ID
        let id_bytes: [u8; 2] = slice[1..3].try_into().map_err(|_| Error::TooFewBytes)?;
        let id = u16::from_le_bytes(id_bytes);

        // Parse length
        let len_bytes: [u8; 2] = slice[3..5].try_into().map_err(|_| Error::TooFewBytes)?;
        let length = u16::from_le_bytes(len_bytes);

        // Parse end-of-header marker
        if slice[5] != END_OF_HEADER {
            return Err(Error::InvalidHeader);
        }

        Self::from_raw(id, length)
    }
}

impl<const N: usize> Frame<N> {
    pub fn new(id: u16, payload: &[u8]) -> Result<Self, Error> {
        let header = FrameHeader::new(id, payload.len())?;

        Ok({
            // pre-initialize
            let mut s = Self {
                header,
                data: [0; N],
            };

            // copy data
            for (i, byte) in payload.iter().enumerate() {
                s.data[i] = *byte;
            }

            s
        })
    }

    pub fn id(&self) -> u16 {
        self.header.id
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data[0..self.header.payload_len()]
    }
}

// try_from header + slice
impl<const N: usize> TryFrom<(FrameHeader, &[u8])> for Frame<N> {
    type Error = Error;

    fn try_from(header_and_bytes: (FrameHeader, &[u8])) -> Result<Self, Self::Error> {
        let (header, b64_data) = header_and_bytes;

        if N < header.payload_len() {
            return Err(Error::TooManyBytes);
        }

        let b64_len = header.data_len();
        if b64_data.len() < b64_len {
            return Err(Error::TooFewBytes);
        }
        if b64_data.len() > b64_len {
            return Err(Error::TooManyBytes);
        }

        let mut frame = Self {
            header,
            data: [0; N],
        };

        // No data to decode: frame is done
        if b64_len == 0 {
            return Ok(frame);
        }

        // Last few bytes may not fit in the output buffer as the encoded data contain 2 extra bytes of CRC checksum.
        // In base64 this is not guaranteed to be at a byte boundary, so we have to decode the last few bytes of data carefully!
        let split_offset = if b64_len < 8 {
            0
        } else {
            (b64_len - 4) & !3 // boundary at multiple of 4: 4 characters decode into exactly 3 bytes
        };

        // Decode bulk of the data directly into frame
        let base64_cfg = base64::Config::new(base64::CharacterSet::Standard, false);
        let bulk_decoded_size =
            base64::decode_config_slice(&b64_data[0..split_offset], base64_cfg, &mut frame.data)
                .map_err(|_| Error::InvalidBase64)?;

        // Decode last few bytes including CRC checksum
        let mut last_bytes: [u8; 8] = [0; 8];
        let remaining_len =
            base64::decode_config_slice(&b64_data[split_offset..], base64_cfg, &mut last_bytes)
                .map_err(|_| Error::InvalidBase64)?;

        if remaining_len < 2 {
            return Err(Error::TooFewBytes);
        }

        // Copy remaining data to frame
        let remaining_data_len = remaining_len - 2;
        let total_data_len = bulk_decoded_size + remaining_data_len;
        assert!(total_data_len == frame.header.payload_len());
        for (i, byte) in last_bytes[0..remaining_data_len].iter().enumerate() {
            frame.data[bulk_decoded_size + i] = *byte;
        }

        // Parse CRC
        let crc_bytes = &last_bytes[remaining_data_len..remaining_len];
        let crc_bytes: [u8; 2] = crc_bytes.try_into().map_err(|_| Error::TooFewBytes)?;
        let parsed_crc = u16::from_le_bytes(crc_bytes);

        // Verify CRC
        // CRC16 checksum is calculated over all binary payload data
        let crc = Crc::<u16>::new(&CRC_16_USB);
        let mut checksum = crc.digest();
        let len = frame.header.payload_len();
        checksum.update(&frame.data[..len]);
        let checksum = checksum.finalize();

        if parsed_crc != checksum {
            return Err(Error::InvalidCRC);
        }

        Ok(frame)
    }
}

// try_from slice
impl<const N: usize> TryFrom<&[u8]> for Frame<N> {
    type Error = Error;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        let header: FrameHeader = slice[0..6].try_into()?;
        let b64_data = &slice[6..];

        Self::try_from((header, b64_data))
    }
}

// try_from ref to array
impl<const N: usize, const L: usize> TryFrom<&[u8; L]> for Frame<N> {
    type Error = Error;

    fn try_from(value: &[u8; L]) -> Result<Self, Self::Error> {
        let slice: &[u8] = value;
        Self::try_from(slice)
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, END_OF_HEADER, START_OF_FRAME};
    use crate::error::Error;

    fn valid_frame_bytes() -> [u8; 13] {
        [
            // Frame header
            START_OF_FRAME, // Start-of-frame marker
            0x37,           // packet ID 0x1337 as little-endian (low byte)
            0x13,           // packet ID 0x1337 as little-endian (high byte)
            0x07,           // Packet length 7 (4-byte data + 3-byte CRC) (low byte)
            0x00,           // Packet length 7 (4-byte data + 3-byte CRC) (high byte)
            END_OF_HEADER,  // End-of-header marker
            // base64-encoded [00, 01, 02] should be "AAEC" = [0x41, 0x41, 0x45, 0x43]
            0x41,
            0x41,
            0x45,
            0x43,
            // CRC16-USB over [00, 01, 02] should be 0x6E0E = [0x0E, 0x6E] (little-endian) = "Dm4"
            0x44,
            0x6D,
            0x34,
        ]
    }

    #[test]
    fn valid_new() {
        // Should be a valid frame containing 3 bytes
        let frame: Frame<3> = Frame::new(0x1337, &[0, 1, 2]).expect("Valid frame");
        assert_eq!(0x1337, frame.id());
        assert_eq!(3, frame.bytes().len());
        assert_eq!(0, frame.bytes()[0]);
        assert_eq!(1, frame.bytes()[1]);
        assert_eq!(2, frame.bytes()[2]);
    }

    #[test]
    fn valid_from_bytes() {
        let frame = valid_frame_bytes();

        // Should be a valid frame containing 3 bytes
        let _frame: Frame<3> = Frame::try_from(&frame).expect("Valid frame");
    }

    #[test]
    fn invalid_start_of_frame_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[0] = 0xF2; // invalid start-of-frame

        let err = Frame::<128>::try_from(&frame).expect_err("Should not be a valid frame header");
        assert_eq!(Error::InvalidHeader, err);
    }
    #[test]
    fn invalid_end_of_header_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[5] = START_OF_FRAME; // invalid end-of-header

        let err = Frame::<128>::try_from(&frame).expect_err("Should not be a valid frame header");
        assert_eq!(Error::InvalidHeader, err);
    }

    #[test]
    fn invalid_id_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[2] = START_OF_FRAME; // invalid ID: MSB cannot go >= 0xF0

        let err = Frame::<128>::try_from(&frame).expect_err("Should not be a valid frame header");
        assert_eq!(Error::InvalidID, err);
    }

    #[test]
    fn invalid_length_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[4] = START_OF_FRAME; // invalid Length: MSB cannot go >= 0xF0

        let err = Frame::<128>::try_from(&frame).expect_err("Should not be a valid frame header");
        assert_eq!(Error::InvalidLength, err);
    }

    #[test]
    fn invalid_length2_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[3] = 6; // wrong length: actual data is 7 bytes

        let err = Frame::<128>::try_from(&frame).expect_err("Should not be a valid frame header");
        assert_eq!(Error::TooManyBytes, err);
    }

    #[test]
    fn frame_too_small_from_bytes() {
        let frame = valid_frame_bytes();

        // Frame defined impossibly small
        let err = Frame::<1>::try_from(&frame).expect_err("Should not be a valid frame header");
        assert_eq!(Error::TooManyBytes, err);
    }

    #[test]
    fn frame_too_small2_from_bytes() {
        let frame = valid_frame_bytes();

        // Frame defined one byte too small
        let err = Frame::<2>::try_from(&frame).expect_err("Should not be a valid frame header");
        assert_eq!(Error::TooManyBytes, err);
    }

    #[test]
    fn invalid_crc_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[6] = 0x42; // corrupt first byte

        let err = Frame::<128>::try_from(&frame).expect_err("CRC should mismatch!");
        assert_eq!(Error::InvalidCRC, err);
    }
    #[test]
    fn invalid_crc2_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[9] = 0x42; // corrupt last byte

        let err = Frame::<128>::try_from(&frame).expect_err("CRC should mismatch!");
        assert_eq!(Error::InvalidCRC, err);
    }
    #[test]
    fn invalid_crc3_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[11] = 0x42; // corrupt CRC byte

        let err = Frame::<128>::try_from(&frame).expect_err("CRC should mismatch!");
        assert_eq!(Error::InvalidCRC, err);
    }

    #[test]
    fn invalid_base64_from_bytes() {
        let mut frame = valid_frame_bytes();
        frame[11] = 0x80; // invalid base64 character

        let err = Frame::<128>::try_from(&frame).expect_err("CRC should mismatch!");
        assert_eq!(Error::InvalidBase64, err);
    }
}
