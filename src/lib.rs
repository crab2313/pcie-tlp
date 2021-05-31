/*!
This crate is one of my attempt to implement a infrastructure aiming to be a bridge
between transaction-level simulated PCIe device and hypervisor. In fact, common
hypervisor's implementation of PCIe devices are simply some kind of register-level
simulation. We need a bridge between transaction-level simulated PCIe device and
hypervisor since this kind of simulated device only speaks PCIe transaction packet.

# Design consideration

1. We should separate the bridge between specific hypervisor and the device
implementation. Make it easy to port the device between different rust-vmm
hypervisor.

2. The simulated devices should run in their own simulation threads for better
isolation. Currently we should just consider one PCIe lane support and we will
expore multiple PCIe lanes eventually. A PCIe lane is simply a pair of stream
of PCIe transaction in our simulation.

3. We should handle PCIe bridging logic in another separated thread. Basically,
our PciAdapter should run inside its own thread. And rely on message passing
between threads to communitcate with other threads.

# Implementation detail

There are basically three roles in the simulation: hypervisor, [`PciAdapter`] and
[`PciSimDevice`]. Conceptually, the adapter and simulated device both running inside
their dedicated thread with some kind of message passing channel for commands and
packet exchaning.

The adapter is some kind of bridge between hypervisor and simulated device. That
is, the adapter exposes two interface both to hypervisor and simulated device.

The simuated device is some kind of PCIe transaction layer level model which speaks
PCIe transactions. Currently, only software based device model is considered. But
RTL based simulation model is also possible.

A typically interfaction between these three roles should look like:

1. When hypervisor need to read a configuration space register, it will
issue a read request to the adapter.

2. Then the adapter translate the request to the standard PCIe type 0 configutation
read transaction and send it to the simulatede device.

3. The simulated device receives the PCIe transaction and send a completion transaction
to the adapter.

4. The adapter should know that this completion is for hypervisor since it just
store the transaction ID it sends before. After that the adapter explicit notifies
the hypervisor and hypervisor should have enough information to continue the execution.
*/

use std::convert::TryFrom;

mod adapter;
mod device;
// mod parser;

pub use adapter::{PciAdapter, PciLane};
pub use device::{PciDeviceCommon, PciSimDevice};

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

/// Packet specific data of config space related PCIe transactions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfigExtra {
    requester: u16,
    completer: u16,
    tag: u8,
    reg: u16,
}

/// Packet specific data of completion PCIe transactions.
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

/// The type of PCIe transaction, tightly coupled with TYPE\[4:0\] and FMT\[2:0\]
/// fields in the header.
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

/// Traffica class of PCIe packet. Byte 1 bits 6:4 of the header.
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// The address type field inside the PCIe transacton headers.
#[derive(Debug, Clone, Copy)]
pub enum AddressType {
    Default = 0b00,
    TranslationRequest,
    Translated,
    Reserved,
}

/// Literally a in memory representation of PCIe transaction headers.
#[derive(Debug, Clone, Copy)]
pub struct TlpHeader {
    _type: PacketType,
    trafic_class: TrafficClass,
    address_type: AddressType,

    /// Attr\[1\]
    relax_ordering: bool,
    /// Attr\[0\]
    no_snoop: bool,
    /// Attr\[2\]
    id_ordering: bool,

    poisoned_data: bool,
    tlp_digest: bool,
    processing_hint: bool,

    // The upper 4 bits is the last DW, and the lower 4 bits are the first DW.
    byte_enable: u8,
    length: u16,
}

/// Basic abstraction of a TLP packet without CRC checksum attached.
#[derive(Debug, Clone)]
pub struct Tlp {
    pub header: TlpHeader,
    pub data: Option<Vec<u32>>,
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

impl Default for Tlp {
    fn default() -> Self {
        Tlp {
            header: TlpHeader::default(),
            data: None,
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
            byte_enable: 0,
            length: 0,
        }
    }
}
/// Convenient builder of [`Tlp`].
#[derive(Debug)]
pub struct TlpBuilder(Tlp);

impl TlpBuilder {
    pub fn with_type(ptype: PacketType) -> Self {
        TlpBuilder(Tlp::default()).r#type(ptype)
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
        Self::with_type(PacketType::Config0Read(extra))
    }

    pub fn config0_write(extra: ConfigExtra) -> Self {
        Self::with_type(PacketType::Config0Write(extra)).length(1)
    }

    pub fn completion_data(extra: CompletionExtra) -> Self {
        Self::with_type(PacketType::CompletionData(extra))
    }

    fn r#type(mut self, _type: PacketType) -> Self {
        self.0.header._type = _type;
        self
    }

    pub fn length(mut self, len: u16) -> Self {
        self.0.header.length = len;
        self
    }

    pub fn data(mut self, data: Vec<u32>) -> Self {
        self.0.data = Some(data);
        self
    }

    pub fn byte_enable(mut self, be: u8) -> Self {
        self.0.header.byte_enable = be;
        self
    }

    pub fn build(self) -> Tlp {
        self.0
    }
}

impl Tlp {
    /// Check whether a TLP is valid according to the PCIe specification.
    pub fn is_valid(&self) -> bool {
        use PacketType::*;

        let header = self.header;

        match header._type {
            Config0Read(extra) | Config0Write(extra) | Config1Read(extra) | Config1Write(extra) => {
                // PCIe 3.0 specification 2.2.7
                if header.trafic_class != TrafficClass::TC0
                    || header.no_snoop
                    || header.relax_ordering
                    || header.length != 0b00001
                    || header.byte_enable & 0xf0 != 0
                {
                    return false;
                }
            }
            _ => unimplemented!(),
        }

        true
    }
}
