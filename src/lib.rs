use std::convert::TryFrom;

mod parser;

/// Byte 0 bits 7:5
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fmt {
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
pub enum PacketType {
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
    Unknown,
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

impl From<PacketType> for u8 {
    fn from(p: PacketType) -> Self {
        use PacketType::*;
        match p {
            MemoryRead | MemoryWrite => 0b00000,
            MemoryReadLock => 0b00001,
            IoRead | IoWrite => 0b00010,
            Config0Read | Config0Write => 0b00100,
            Config1Read | Config1Write => 0b00101,
            Message(r) | MessageData(r) => 0b10000 | (r & 0b111),
            Completion | CompletionData => 0b01010,
            CompletionLocked | CompletionLockedData => 0b01011,
            FetchAddAtomic => 0b01100,
            SwapAtomic => 0b01101,
            CasAtomic => 0b01110,
            LocalPrefix(l) => 0b1111 & l,
            EndToEndPrefix(e) => 0b10000 | (0b1111 & e),
            Unknown => panic!("unknown packet type"),
        }
    }
}

/// Byte 1 bits 6:4
#[derive(Debug, Clone, Copy)]
pub enum TrafficClass {
    TC0 = 0b000,
    TC1,
    TC2,
    TC3,
    TC4,
    TC5,
    TC6,
    TC7,
}

#[derive(Debug, Clone, Copy)]
pub enum AddressType {
    Default = 0b00,
    TranslationRequest,
    Translated,
    Reserved,
}

/// Basic abstraction of the TLP packet
/// For now I just put all fields common to all types of TLP into the struct.
/// And put the type specific part into the `_type` enum. That's fairly enough.
/// We should also consider other types of advanced feature such as TLP validation.
pub struct TlpPacket<'a> {
    _type: PacketType,
    fmt: Fmt,
    trafic_class: TrafficClass,
    address_type: AddressType,

    // three attributes
    relax_ordering: bool,
    no_snoop: bool,
    id_ordering: bool,

    poisoned_data: bool,
    tlp_digest: bool,
    processing_hint: bool,

    /// The upper 4 bits is the last DW, and the lower 4 bits are the first DW.
    dw: u8,
    length: u16,
    data: Option<&'a [u8]>,
}

impl<'a> Default for TlpPacket<'a> {
    fn default() -> Self {
        TlpPacket {
            _type: PacketType::Unknown,
            fmt: Fmt::Dw3NoData,
            trafic_class: TrafficClass::TC0,
            address_type: AddressType::Default,
            relax_ordering: false,
            no_snoop: false,
            id_ordering: false,
            poisoned_data: false,
            processing_hint: false,
            tlp_digest: false,
            dw: 0,
            length: 0,
            data: None,
        }
    }
}

// TODO: builder pattern to build various types of packet
// TODO: TlpBuilder: convenient helper to build the TLP

impl<'a> TlpPacket<'a> {
    pub fn memory_read() -> Self {
        let mut tlp = TlpPacket::default();
        tlp._type(PacketType::MemoryRead);
        tlp
    }

    fn _type(&mut self, _type: PacketType) -> &mut Self {
        self._type = _type;
        self
    }
}

impl<'a> TlpPacket<'a> {
    pub fn header(&self) -> Vec<u8> {
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
