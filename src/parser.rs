use crate::*;
use nom::error::ErrorKind;
use nom::error::ParseError;
use nom::number::streaming::u8;
use nom::Err::Error;
use nom::IResult;

#[derive(Debug, PartialEq)]
pub enum CustomError<I> {
    InvalidHeader,
    Nom(I, ErrorKind),
}

impl<I> ParseError<I> for CustomError<I> {
    fn from_error_kind(input: I, kind: ErrorKind) -> Self {
        CustomError::Nom(input, kind)
    }

    fn append(_: I, _: ErrorKind, other: Self) -> Self {
        other
    }
}

fn header(i: &[u8]) -> IResult<&[u8], PacketFormat, CustomError<&[u8]>> {
    let (i, b0) = u8(i)?;
    let (i, b1) = u8(i)?;
    let (i, b2) = u8(i)?;
    let (i, b3) = u8(i)?;

    let format = PacketFormat::try_from(b0).map_err(|_| Error(CustomError::InvalidHeader))?;

    let length = (((b2 & 0b11) as usize) << 8) + b3 as usize;
    let relax_ordering = b2 & 0b100000 != 0;
    let no_snoop = b2 & 0b10000 != 0;
    let ph = b1 & 0b1 != 0;
    let traffic_class = (b1 >> 4) & 0b111;

    Ok((i, format))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head() {
        let data = &[0b00110110u8, 0b0, 0b0, 0b0];
        assert!(header(data).is_ok());
    }
}
