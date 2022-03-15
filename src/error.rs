#[derive(Debug)]
pub enum Error {
    InvalidID,
    InvalidLength,
    InvalidCRC,
    QueueOverflow,
}
