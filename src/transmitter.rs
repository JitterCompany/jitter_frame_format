use core::cmp::min;

use crate::{error::Error, frame};
use crc::{Crc, CRC_16_USB};

pub struct Transmitter<TX> {
    tx: TX,
}

pub trait TransmitQueue {
    fn space_available(&self) -> usize;
    fn write(&mut self, byte: u8) -> Result<(), ()>;
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

        // Process data in blocks so we can handle arbitrary input data length
        // NB: BLOCK_SIZE must be a multiple of 3 (3 bytes encode into exactly 4 output characters)
        const BLOCK_SIZE: usize = 30;
        let base64_cfg = base64::Config::new(base64::CharacterSet::Standard, false);
        for offset in (0..data.len()).step_by(BLOCK_SIZE) {
            let end_index = min(data.len(), offset + BLOCK_SIZE);
            let input = &data[offset..end_index];
            checksum.update(input);

            let mut encoded: [u8; BLOCK_SIZE * 2] = [0; BLOCK_SIZE * 2];
            let encoded_size = base64::encode_config_slice(input, base64_cfg, &mut encoded);

            // Write base64-encoded data
            for byte in &encoded[0..encoded_size] {
                self.write(*byte)?;
            }
        }

        // Write CRC16
        let checksum = checksum.finalize().to_le_bytes();
        for byte in checksum {
            self.write(byte)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{TransmitQueue, Transmitter};

    struct DummyTransmitter<'a> {
        data: &'a mut [u8; 0xFFFF],
        tx_count: &'a mut usize,
    }
    impl TransmitQueue for DummyTransmitter<'_> {
        fn space_available(&self) -> usize {
            0xFFFF_usize - *self.tx_count
        }

        fn write(&mut self, byte: u8) -> Result<(), ()> {
            if *self.tx_count >= 0xFFFF_usize {
                return Err(());
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
        assert_eq!(12, tx_count, "Expect 12-byte message"); // 6-byte header, 4-byte data, 2-byte CRC

        // Frame header
        assert_eq!(data[0], 0xF1); // Start-of-frame marker
        assert_eq!(data[1], 0x37); // packet ID 0x1337 as little-endian (low byte)
        assert_eq!(data[2], 0x13); // packet ID 0x1337 as little-endian (high byte)
        assert_eq!(data[3], 0x06); // Packet length 6 (4-byte data + 2-byte CRC) (low byte)
        assert_eq!(data[4], 0x00); // Packet length 6 (4-byte data + 2-byte CRC) (high byte)
        assert_eq!(data[5], 0xFF); // End-of-header marker

        // base64-encoded [00, 01, 02] should be "AAEC" = [0x41, 0x41, 0x45, 0x43]
        assert_eq!(data[6], 0x41);
        assert_eq!(data[7], 0x41);
        assert_eq!(data[8], 0x45);
        assert_eq!(data[9], 0x43);

        // CRC16-USB over [00, 01, 02] should be 0x6E0E = [0x0E, 0x6E] (little-endian)
        assert_eq!(data[10], 0x0E);
        assert_eq!(data[11], 0x6E);
    }
}
