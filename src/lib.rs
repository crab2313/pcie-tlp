use std::convert::TryFrom;

mod adapter;
mod device;
// mod parser;

pub use adapter::{PciAdapter, PciLane};
pub use device::PciSimDevice;

/// Byte 0 bits 7:5
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Fmt {
    Dw3NoData = 0b00 << 5,
    Dw4NoData = 0b01 << 5,
    Dw3 = 0b10 << 5,
    Dw4 = 0b11 << 5,
    Prefix = 0b100 << 5,
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

// After a glance of others' implementations of PCIe TLP simulation, I found that
// the FMT & TYPE could uniquely identify a type of packet. That reminds me to
// redesign the representation of packet thoroughly.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfigExtra {
    requester: u16,
    completer: u16,
    tag: u8,
    reg: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompletionExtra {
    requester: u16,
    completer: u16,
    tag: u8,
    status: u8,
    bcm: bool,
    byte_count: u16,
    lower_address: u8,
}

/// The type of the tlp, tightly coupled with TYPE[4:0] field and FMT[2:0]
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum PacketType {
    MemoryRead,
    MemoryRead64,
    MemoryReadLock,
    MemoryReadLock64,
    MemoryWrite,
    MemoryWrite64,
    IoRead,
    IoWrite,
    Config0Read(ConfigExtra),
    Config0Write(ConfigExtra),
    Config1Read(ConfigExtra),
    Config1Write(ConfigExtra),
    Message(u8),
    MessageData(u8),
    Completion(CompletionExtra),
    CompletionData(CompletionExtra),
    CompletionLocked(CompletionExtra),
    CompletionLockedData(CompletionExtra),
    FetchAddAtomic,
    SwapAtomic,
    CasAtomic,
    LocalPrefix(u8),
    EndToEndPrefix(u8),
    Unknown,
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
#[derive(Debug, Copy, Clone)]
pub struct Tlp<'a> {
    header: TlpHeader,
    data: Option<&'a [u8]>,
}

#[derive(Debug, Clone, Copy)]
pub struct TlpHeader {
    _type: PacketType,
    trafic_class: TrafficClass,
    address_type: AddressType,

    // three attributes
    relax_ordering: bool,
    no_snoop: bool,
    id_ordering: bool,

    poisoned_data: bool,
    tlp_digest: bool,
    processing_hint: bool,

    // The upper 4 bits is the last DW, and the lower 4 bits are the first DW.
    dw: u8,
    length: u16,
}

impl TlpHeader {
    fn transaction_id(&self) -> u32 {
        use PacketType::*;

        match self._type {
            Config0Read(extra) | Config0Write(extra) => {
                extra.tag as u32 | ((extra.requester as u32) << 16)
            }
            CompletionData(extra) | Completion(extra) => {
                extra.tag as u32 | ((extra.requester as u32) << 16)
            }
            _ => unimplemented!(),
        }
    }
}

impl Default for TlpHeader {
    fn default() -> Self {
        TlpHeader {
            _type: PacketType::Unknown,
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
        }
    }
}

// TODO: builder pattern to build various types of packet
// TODO: TlpBuilder: convenient helper to build the TLP
#[derive(Debug, Clone, Copy)]
pub struct TlpHeaderBuilder(TlpHeader);

impl TlpHeaderBuilder {
    pub fn with_type(ptype: PacketType) -> Self {
        let mut builder = TlpHeaderBuilder(TlpHeader::default());
        builder.r#type(ptype);
        builder
    }

    pub fn memory_read() -> Self {
        Self::with_type(PacketType::MemoryRead)
    }

    pub fn io_read() -> Self {
        Self::with_type(PacketType::IoRead)
    }

    pub fn io_write() -> Self {
        Self::with_type(PacketType::IoWrite)
    }

    pub fn config0_read(extra: ConfigExtra) -> Self {
        *Self::with_type(PacketType::Config0Read(extra)).length(1)
    }

    pub fn completion_data(extra: CompletionExtra) -> Self {
        Self::with_type(PacketType::CompletionData(extra))
    }

    fn r#type(&mut self, _type: PacketType) -> &mut Self {
        self.0._type = _type;
        self
    }

    pub fn build(self) -> TlpHeader {
        self.0
    }

    pub fn length(&mut self, len: u16) -> &mut Self {
        self.0.length = len;
        self
    }
}
