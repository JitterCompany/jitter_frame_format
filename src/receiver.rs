use crate::{
    error::Error,
    frame::{self, FrameHeader, START_OF_FRAME},
};

pub struct Receiver<RX> {
    bytes_skipped: u32,
    rx: RX,
}

pub trait ReceiveQueue {
    fn bytes_available(&self) -> usize;
    fn peek_at(&self, offset: usize) -> Option<u8>;
    fn flush(&mut self, n_bytes: usize);
}

impl<RX> Receiver<RX>
where
    RX: ReceiveQueue,
{
    pub fn new(rx: RX) -> Self {
        Self {
            rx,
            bytes_skipped: 0,
        }
    }

    /// Returns total amount of incoming bytes that were discarded
    ///
    /// The receiver only discards bytes that cannot form a valid frame.
    /// This could happen when recovering after a connection loss / corrupt packet / hotplug situation.
    /// This is a statistic similar to 'packet loss' in networking: substantial amount of bytes are skipped
    /// may indicate a bad link quality
    pub fn bytes_skipped(&self) -> u32 {
        self.bytes_skipped
    }

    fn peek_bytes(
        &self,
        offset: usize,
        n: usize,
        result: &mut [u8],
    ) -> nb::Result<(), crate::error::Error> {
        for i in 0..n {
            match self.rx.peek_at(offset + i) {
                Some(byte) => {
                    result[i] = byte;
                }
                None => {
                    return Err(nb::Error::Other(Error::QueueUnderflow));
                }
            }
        }
        Ok(())
    }

    fn skip_byte(&mut self) {
        self.rx.flush(1);
        self.bytes_skipped += 1;
    }

    fn rx_header(&mut self) -> nb::Result<FrameHeader, crate::error::Error> {
        // Skip bytes untill START_OF_RAME is detected
        loop {
            match self.rx.peek_at(0) {
                None => return Err(nb::Error::WouldBlock),
                Some(START_OF_FRAME) => {
                    break;
                }
                _ => {
                    self.skip_byte();
                }
            }
        }

        // Wait untill enough data is available to form a packet header
        if self.rx.bytes_available() < 6 {
            return Err(nb::Error::WouldBlock);
        }

        // Build a packet header
        let mut header_bytes = [0_u8; 6];
        self.peek_bytes(0, 6, &mut header_bytes[0..])?;
        match FrameHeader::try_from(header_bytes) {
            Ok(header) => Ok(header),
            Err(e) => {
                // Header invalid: skip a byte and try again next time...
                self.skip_byte();
                return Err(nb::Error::Other(e));
            }
        }
    }

    pub fn receive<const N: usize>(&mut self) -> nb::Result<frame::Frame<N>, crate::error::Error> {
        let header = self.rx_header()?;

        // Incoming packet would be too long to receive correctly: discard it
        if header.payload_len() > N {
            self.skip_byte();
            return Err(nb::Error::Other(Error::TooManyBytes));
        }

        // Wait untill enough data is available to form a complete packet
        let total_len = header.total_packet_len();
        if self.rx.bytes_available() < total_len {
            return Err(nb::Error::WouldBlock);
        }

        let mut data = [0_u8; N];
        let data_len = header.data_len();
        self.peek_bytes(6, data_len, &mut data[0..])?;
        match frame::Frame::try_from((header, &data[0..data_len])) {
            Ok(frame) => {
                self.rx.flush(total_len);
                Ok(frame)
            }
            Err(e) => {
                // Frame invalid: skip a byte and try again next time...
                self.skip_byte();
                Err(nb::Error::Other(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::{Frame, END_OF_HEADER, START_OF_FRAME};

    use super::{ReceiveQueue, Receiver};

    struct DummyReceiver<'a> {
        data: &'a [u8],
        rx_count: &'a mut usize,
    }
    impl ReceiveQueue for DummyReceiver<'_> {
        fn bytes_available(&self) -> usize {
            self.data.len() - *self.rx_count
        }

        fn peek_at(&self, offset: usize) -> Option<u8> {
            let read_offset = *self.rx_count;
            if offset < self.data.len() {
                Some(self.data[read_offset + offset])
            } else {
                panic!("DummyReceiver: peek past end of data!");
            }
        }

        fn flush(&mut self, n_bytes: usize) {
            *self.rx_count += n_bytes;
        }
    }

    #[test]
    fn receive_works() {
        let mut data = [
            START_OF_FRAME,
            0x37,
            0x13,
            0x07,
            0x00,
            END_OF_HEADER,
            0x41,
            0x41,
            0x45,
            0x43,
            0x44,
            0x6D,
            0x34,
        ];
        let mut rx_count: usize = 0;
        let rx = DummyReceiver {
            data: &mut data,
            rx_count: &mut rx_count,
        };
        let mut receiver = Receiver::new(rx);
        let frame: Frame<128> = receiver.receive().expect("Receive failed!");

        assert_eq!(0x1337, frame.id());
        assert_eq!(3, frame.bytes().len());
        assert_eq!(0, frame.bytes()[0]);
        assert_eq!(1, frame.bytes()[1]);
        assert_eq!(2, frame.bytes()[2]);
        assert_eq!(0, receiver.bytes_skipped());
    }

    #[test]
    fn receive_skip1_works() {
        let data = [
            0x34,
            START_OF_FRAME,
            0x37,
            0x13,
            0x07,
            0x00,
            END_OF_HEADER,
            0x41,
            0x41,
            0x45,
            0x43,
            0x44,
            0x6D,
            0x34,
        ];
        let mut rx_count: usize = 0;
        let rx = DummyReceiver {
            data: &data,
            rx_count: &mut rx_count,
        };
        let mut receiver = Receiver::new(rx);
        let frame: Frame<128> = receiver.receive().expect("Receive failed!");

        assert_eq!(0x1337, frame.id());
        assert_eq!(3, frame.bytes().len());
        assert_eq!(0, frame.bytes()[0]);
        assert_eq!(1, frame.bytes()[1]);
        assert_eq!(2, frame.bytes()[2]);
        assert_eq!(1, receiver.bytes_skipped());
    }

    #[test]
    fn receive_skip_any_works() {
        let data = [
            0x37,
            0x13,
            0x07,
            0x00,
            END_OF_HEADER,
            0x41,
            0x41,
            0x45,
            0x43,
            0x44,
            0x6D,
            0x34,
            START_OF_FRAME,
            0x37,
            0x13,
            0x07,
            0x00,
            END_OF_HEADER,
            0x41,
            0x41,
            0x45,
            0x43,
            0x44,
            0x6D,
            0x34,
        ];

        for offset in 0..12 {
            let mut test_data = data;
            test_data[offset] = START_OF_FRAME;
            let test_data = &test_data[offset..];
            let mut rx_count: usize = 0;
            let rx = DummyReceiver {
                data: test_data,
                rx_count: &mut rx_count,
            };
            assert_eq!(START_OF_FRAME, test_data[0]);
            let mut receiver = Receiver::new(rx);
            let _e = receiver
                .receive::<128>()
                .expect_err("Invalid data should be skipped");
            let frame = receiver.receive::<128>().expect("Valid");

            assert_eq!(0x1337, frame.id());
            assert_eq!(3, frame.bytes().len());
            assert_eq!(0, frame.bytes()[0]);
            assert_eq!(1, frame.bytes()[1]);
            assert_eq!(2, frame.bytes()[2]);
        }
    }
}
