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

const MEMORY_READ: u8 = Fmt::Dw3NoData as u8 | 0b00000;
const MEMORY_READ_64: u8 = Fmt::Dw4NoData as u8 | 0b00000;

//MemoryReadLock = Fmt::Dw3NoData as u16 | 0b00001,
//MemoryReadLock64 = Fmt::Dw4NoData as u16 | 0b00001,
//MemoryWrite = Fmt::Dw3 as u16 | 0b00000,
//MemoryWrite64 = Fmt::Dw4 as u16 | 0b00000,
//IoRead = Fmt::Dw3NoData as u16 | 0b00010,
//IoWrite = Fmt::Dw3 as u16 | 0b00010,
const CONFIG0_READ: u8 = Fmt::Dw3NoData as u8 | 0b00100;
const CONFIG9_WRITE: u8 = Fmt::Dw3 as u8 | 0b00100;

//Config1Read = Fmt::Dw3NoData as u16 | 0b00101,
//Config1Write = Fmt::Dw3 as u16 | 0b00101,

fn header(i: &[u8]) -> IResult<&[u8], PacketFormat, CustomError<&[u8]>> {
    let (i, b0) = u8(i)?;
    let (i, b1) = u8(i)?;
    let (i, b2) = u8(i)?;
    let (i, b3) = u8(i)?;

    let config_extra = ConfigExtra {
        requester:
    }

    use PacketType::*;

    let r#type = match b0 {
        CONFIG0_READ => {
            Config0Read(
                ConfigExtra {
                    requester
                }
            )
        },
        _ => unimplemented!(),
    };

    let format = PacketFormat::try_from(b0).map_err(|_| Error(CustomError::InvalidHeader))?;

    let length = (((b2 & 0b11) as usize) << 8) + b3 as usize;
    let relax_ordering = b2 & 0b100000 != 0;
    let no_snoop = b2 & 0b10000 != 0;
    let ph = b1 & 0b1 != 0;
    let traffic_class = (b1 >> 4) & 0b111;

    Ok((i, format))
}

impl TlpHeader {
    fn to_buffer(&self) -> Vec<u8> {
        let len = match self.fmt {
            Fmt::Dw3 | Fmt::Dw3NoData => 12,
            Fmt::Dw4 | Fmt::Dw4NoData => 16,
            _ => unreachable!(),
        };

        let mut header = vec![0; len];

        // let's construct the fixed part of header
        header[0] = u8::from(self._type) | ((self.fmt as u8) << 5);
        header[1] = (self.processing_hint as u8)
            | ((self.id_ordering as u8) << 2)
            | ((self.trafic_class as u8) << 4);
        header[2] = ((self.length >> 8) & 0b11) as u8
            | ((self.address_type as u8) << 2)
            | ((self.no_snoop as u8) << 4)
            | ((self.relax_ordering as u8) << 5)
            | ((self.poisoned_data as u8) << 6)
            | ((self.tlp_digest as u8) << 7);
        header[3] = self.length as u8;
        header[7] = self.dw;

        // TODO: packet type specific part of header fields
        header
    }
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
