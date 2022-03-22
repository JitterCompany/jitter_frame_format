#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidHeader,
    InvalidID,
    InvalidLength,
    InvalidCRC,
    InvalidBase64,
    QueueUnderflow,
    QueueOverflow,
    TooManyBytes,
    TooFewBytes,
}
