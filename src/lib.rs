use std::convert::TryFrom;

mod parser;

/// Byte 0 bits 7:5
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Fmt {
    Dw3NoData = 0b00,
    Dw4NoData = 0b01,
    Dw3 = 0b10,
    Dw4 = 0b11,
    Prefix = 0b100,
}

impl TryFrom<u8> for Fmt {
    type Error = ();
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0b00 => Ok(Fmt::Dw3NoData),
            0b01 => Ok(Fmt::Dw4NoData),
            0b10 => Ok(Fmt::Dw3),
            0b11 => Ok(Fmt::Dw4),
            0b100 => Ok(Fmt::Prefix),
            _ => Err(()),
        }
    }
}
/// The type of the tlp, tightly coupled with TYPE[4:0] field and FMT[2:0]
#[derive(Debug, PartialEq, Clone, Copy)]
enum PacketType {
    MemoryRead,
    MemoryReadLock,
    MemoryWrite,
    IoRead,
    IoWrite,
    Config0Read,
    Config0Write,
    Config1Read,
    Config1Write,
    Message(u8),
    MessageData(u8),
    Completion,
    CompletionData,
    CompletionLocked,
    CompletionLockedData,
    FetchAddAtomic,
    SwapAtomic,
    CasAtomic,
    LocalPrefix(u8),
    EndToEndPrefix(u8),
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct PacketFormat {
    _type: PacketType,
    fmt: Fmt,
}

impl TryFrom<u8> for PacketFormat {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        use Fmt::*;
        use PacketType::*;

        let _type = value & 0b11111;
        let fmt = Fmt::try_from(value >> 5)?;

        Ok(PacketFormat {
            _type: match (fmt, _type) {
                (Dw3NoData, 0b00000) | (Dw4NoData, 0b00000) => Ok(MemoryRead),
                (Dw3NoData, 0b00001) | (Dw4NoData, 0b00001) => Ok(MemoryReadLock),
                (Dw3, 0b00000) | (Dw4, 0b00000) => Ok(MemoryWrite),
                (Dw3NoData, 0b00010) => Ok(IoRead),
                (Dw3, 0b00010) => Ok(IoWrite),
                (Dw3NoData, 0b00100) => Ok(Config0Read),
                (Dw3, 0b00100) => Ok(Config0Write),
                (Dw3NoData, 0b00101) => Ok(Config1Read),
                (Dw3, 0b00101) => Ok(Config1Write),
                (Dw4NoData, _) if _type >> 3 == 0b10 => Ok(Message(_type & 0b111)),
                (Dw4, _) if _type >> 3 == 0b10 => Ok(MessageData(_type & 0b111)),
                (Dw3NoData, 0b01010) => Ok(Completion),
                (Dw3, 0b01010) => Ok(CompletionData),
                (Dw3NoData, 0b01011) => Ok(CompletionLocked),
                (Dw3, 0b01011) => Ok(CompletionLockedData),
                (Dw3, 0b01100) | (Dw4, 0b01100) => Ok(FetchAddAtomic),
                (Dw3, 0b01101) | (Dw4, 0b01101) => Ok(SwapAtomic),
                (Dw3, 0b01110) | (Dw4, 0b01110) => Ok(CasAtomic),
                (Prefix, _) if _type >> 4 == 0 => Ok(LocalPrefix(_type & 0b1111)),
                (Prefix, _) if _type >> 4 == 1 => Ok(EndToEndPrefix(_type & 0b1111)),
                _ => Err(()),
            }?,
            fmt,
        })
    }
}

/// Byte 1 bits 6:4
enum TrafficClass {}

enum AddressType {
    Default,
    TranslationRequest,
    Translated,
    Reserved,
}

pub struct TlpPacket {
    _type: PacketType,
}

#[cfg(test)]
mod tests {
    use crate::*;
    #[test]
    fn packet_type() {
        let format = PacketFormat::try_from(0b01101100u8).unwrap();
        assert_eq!(
            format,
            PacketFormat {
                _type: PacketType::FetchAddAtomic,
                fmt: Fmt::Dw4
            }
        );

        let format = PacketFormat::try_from(0b00110110u8).unwrap();
        assert_eq!(
            format,
            PacketFormat {
                _type: PacketType::Message(0b110),
                fmt: Fmt::Dw4NoData
            }
        );

        assert!(PacketFormat::try_from(0b01010110).is_err());
    }
}
