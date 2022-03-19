#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidHeader,
    InvalidID,
    InvalidLength,
    InvalidCRC,
    InvalidBase64,
    QueueOverflow,
    TooManyBytes,
    TooFewBytes,
}
