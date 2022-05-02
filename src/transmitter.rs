use crate::{error::Error, frame};
use crc::{Crc, CRC_16_USB};

pub struct Transmitter<TX> {
    tx: TX,
}

pub trait TransmitQueue {
    fn space_available(&self) -> usize;
    fn write(&mut self, byte: u8) -> Result<(), u8>;
}

impl<TX> Transmitter<TX>
where
    TX: TransmitQueue,
{
    pub fn new(tx: TX) -> Self {
        Self { tx }
    }

    fn write(&mut self, byte: u8) -> nb::Result<(), crate::error::Error> {
        match self.tx.write(byte) {
            Ok(_) => Ok(()),
            Err(_) => return Err(nb::Error::Other(Error::QueueOverflow)),
        }
    }

    pub fn transmit_frame<const N: usize>(
        &mut self,
        frame: &frame::Frame<N>,
    ) -> nb::Result<(), crate::error::Error> {
        self.transmit(frame.id(), frame.bytes())
    }

    pub fn transmit(&mut self, packet_id: u16, data: &[u8]) -> nb::Result<(), crate::error::Error> {
        let header = match frame::FrameHeader::new(packet_id, data.len()) {
            Ok(frame) => frame,
            Err(e) => return Err(nb::Error::Other(e)),
        };

        if self.tx.space_available() < header.total_packet_len() {
            return Err(nb::Error::WouldBlock);
        }

        // Write header
        let header = header.as_bytes();
        for byte in header {
            self.write(byte)?;
        }

        // CRC16 checksum is calculated over all input data (before base64 encoding)
        let crc = Crc::<u16>::new(&CRC_16_USB);
        let mut checksum = crc.digest();
        checksum.update(data);
        let checksum = checksum.finalize().to_le_bytes();

        // Process data in blocks so we can handle arbitrary input data length
        // NB: BLOCK_SIZE must be a multiple of 3 (3 bytes encode into exactly 4 output characters)
        const BLOCK_SIZE: usize = 30;
        let base64_cfg = base64::Config::new(base64::CharacterSet::Standard, false);
        for offset in (0..data.len()).step_by(BLOCK_SIZE) {
            let end_index = offset + BLOCK_SIZE;

            let mut encoded: [u8; BLOCK_SIZE * 2] = [0; BLOCK_SIZE * 2];

            let encoded_size = if (end_index + 1) < data.len() {
                let input = &data[offset..end_index];
                base64::encode_config_slice(input, base64_cfg, &mut encoded)

            // Last block: combine data + CRC before base64 encoding
            } else {
                let input = &data[offset..];
                let in_len = input.len();
                let mut tmp: [u8; BLOCK_SIZE + 2] = [0; BLOCK_SIZE + 2];
                for (i, byte) in input.iter().enumerate() {
                    tmp[i] = *byte;
                }
                tmp[in_len] = checksum[0];
                tmp[in_len + 1] = checksum[1];

                base64::encode_config_slice(&tmp[0..in_len + 2], base64_cfg, &mut encoded)
            };

            // Write base64-encoded data
            for byte in &encoded[0..encoded_size] {
                self.write(*byte)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::{Frame, END_OF_HEADER, START_OF_FRAME};

    use super::{TransmitQueue, Transmitter};

    struct DummyTransmitter<'a> {
        data: &'a mut [u8; 0xFFFF],
        tx_count: &'a mut usize,
    }
    impl TransmitQueue for DummyTransmitter<'_> {
        fn space_available(&self) -> usize {
            0xFFFF_usize - *self.tx_count
        }

        fn write(&mut self, byte: u8) -> Result<(), u8> {
            if *self.tx_count >= 0xFFFF_usize {
                return Err(byte);
            }

            self.data[*self.tx_count] = byte;
            *self.tx_count += 1;
            Ok(())
        }
    }

    #[test]
    fn transmit_works() {
        let mut data = [0; 0xFFFF];
        let mut tx_count: usize = 0;
        let tx = DummyTransmitter {
            data: &mut data,
            tx_count: &mut tx_count,
        };
        let mut transmitter = Transmitter::new(tx);
        transmitter
            .transmit(0x1337, &[0x0, 0x1, 0x2])
            .expect("Transmit failed!");
        assert_eq!(6 + 7, tx_count, "Expect 13-byte message"); // 6-byte header + 8/6 * (3-byte data + 2-byte CRC)

        // Frame header
        assert_eq!(data[0], START_OF_FRAME); // Start-of-frame marker
        assert_eq!(data[1], 0x37); // packet ID 0x1337 as little-endian (low byte)
        assert_eq!(data[2], 0x13); // packet ID 0x1337 as little-endian (high byte)
        assert_eq!(data[3], 0x07); // Packet length 7 (4-byte data + 3-byte CRC) (low byte)
        assert_eq!(data[4], 0x00); // Packet length 7 (4-byte data + 3-byte CRC) (high byte)
        assert_eq!(data[5], END_OF_HEADER); // End-of-header marker

        // base64-encoded [00, 01, 02] should be "AAEC" = [0x41, 0x41, 0x45, 0x43]
        assert_eq!(data[6], 0x41);
        assert_eq!(data[7], 0x41);
        assert_eq!(data[8], 0x45);
        assert_eq!(data[9], 0x43);

        // CRC16-USB over [00, 01, 02] should be 0x6E0E = [0x0E, 0x6E] (little-endian) = "Dm4"
        assert_eq!(data[10], 0x44);
        assert_eq!(data[11], 0x6D);
        assert_eq!(data[12], 0x34);
    }

    #[test]
    /// Same as `transmit_works()` but using the `transmit_frame()` API
    fn transmit_frame_works() {
        let mut data = [0; 0xFFFF];
        let mut tx_count: usize = 0;
        let tx = DummyTransmitter {
            data: &mut data,
            tx_count: &mut tx_count,
        };

        let frame = Frame::<128>::new(0x1337, &[0, 1, 2]).expect("Valid frame");
        let mut transmitter = Transmitter::new(tx);
        transmitter
            .transmit_frame(&frame)
            .expect("Transmit failed!");
        assert_eq!(6 + 7, tx_count, "Expect 13-byte message"); // 6-byte header + 8/6 * (3-byte data + 2-byte CRC)

        // Frame header
        assert_eq!(data[0], START_OF_FRAME); // Start-of-frame marker
        assert_eq!(data[1], 0x37); // packet ID 0x1337 as little-endian (low byte)
        assert_eq!(data[2], 0x13); // packet ID 0x1337 as little-endian (high byte)
        assert_eq!(data[3], 0x07); // Packet length 7 (4-byte data + 3-byte CRC) (low byte)
        assert_eq!(data[4], 0x00); // Packet length 7 (4-byte data + 3-byte CRC) (high byte)
        assert_eq!(data[5], END_OF_HEADER); // End-of-header marker

        // base64-encoded [00, 01, 02] should be "AAEC" = [0x41, 0x41, 0x45, 0x43]
        assert_eq!(data[6], 0x41);
        assert_eq!(data[7], 0x41);
        assert_eq!(data[8], 0x45);
        assert_eq!(data[9], 0x43);

        // CRC16-USB over [00, 01, 02] should be 0x6E0E = [0x0E, 0x6E] (little-endian) = "Dm4"
        assert_eq!(data[10], 0x44);
        assert_eq!(data[11], 0x6D);
        assert_eq!(data[12], 0x34);
    }

    #[test]
    fn transmit_long_packet_works() {
        let mut data = [0; 0xFFFF];
        let mut tx_count: usize = 0;
        let tx = DummyTransmitter {
            data: &mut data,
            tx_count: &mut tx_count,
        };
        let mut transmitter = Transmitter::new(tx);
        transmitter
            .transmit(
                0x1337,
                &[
                    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                    23, 24, 25, 26, 27, 28, 29, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18,
                    17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2,
                ],
            )
            .expect("Transmit failed!");
        // 58 bytes = 78+2 bytes of base64
        assert_eq!(6 + 78 + 2, tx_count, "Expect 80-byte message");
        assert_eq!(0, data[6 + 78 + 2]);
        // Frame header
        assert_eq!(data[0], START_OF_FRAME); // Start-of-frame marker
        assert_eq!(data[1], 0x37); // packet ID 0x1337 as little-endian (low byte)
        assert_eq!(data[2], 0x13); // packet ID 0x1337 as little-endian (high byte)
        assert_eq!(data[3], 78 + 2); // Length of encoded data (low byte)
        assert_eq!(data[4], 0x00); // Length of encoded data (high byte)
        assert_eq!(data[5], END_OF_HEADER); // End-of-header marker

        // (expect CRC = 0x8F53 == 36691)

        // Should be possible to create a valid frame from these bytes
        let _frame: Frame<128> = Frame::try_from(&data[0..6 + 78 + 2]).expect("Invalid packet");
    }

    #[test]
    fn transmit_zero_length_packet_works() {
        let mut data = [0xBE; 0xFFFF];
        let mut tx_count: usize = 0;
        let tx = DummyTransmitter {
            data: &mut data,
            tx_count: &mut tx_count,
        };
        let mut transmitter = Transmitter::new(tx);
        transmitter.transmit(0x1337, &[]).expect("Transmit failed!");
        assert_eq!(6, tx_count, "Expect 6-byte message");

        // Frame header
        assert_eq!(data[0], START_OF_FRAME); // Start-of-frame marker
        assert_eq!(data[1], 0x37); // packet ID 0x1337 as little-endian (low byte)
        assert_eq!(data[2], 0x13); // packet ID 0x1337 as little-endian (high byte)
        assert_eq!(data[3], 0x00); // Length of encoded data (low byte)
        assert_eq!(data[4], 0x00); // Length of encoded data (high byte)
        assert_eq!(data[5], END_OF_HEADER); // End-of-header marker

        assert_eq!(data[6], 0xBE); // Should not be written to

        // Should be possible to create a valid frame from these bytes
        let _frame: Frame<128> = Frame::try_from(&data[0..6]).expect("Invalid packet");
    }
}
