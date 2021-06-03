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

# Core components
In my mind model, there should be three components to enable our simulation.

* A device model which only speaks to outside with PCIe transactions.
* A bridge which is a translator between the device model language (PCIe transactions)
  and hypervisor languages (registers I/O ports and shared memory).
* A handle to enable the hypervisor make requests to the device model.

There should be another layer of abstraction of PCIe lane which is the channel
between bridge and device model. Currently, only software based device model is
considered. But RTL based simulation model is also possible.

There are basically three roles in the crate: hypervisor, [`PciAdapter`] and
[`PciSimDevice`]. Conceptually, the bridge and simulated device both running inside
their dedicated thread with some kind of message passing channel for commands and
packet exchaning. The [`PciAdapter`] is some kind of communication endpoint between
hypervisor and bridge.

A typically interaction between these three roles should look like:

1. When hypervisor need to read a configuration space register, it will
issue a read request to the adapter.

2. Then the adapter translate the request to the standard PCIe type 0 configutation
read transaction and send it to the simulatede device.

3. The simulated device receives the PCIe transaction and send a completion transaction
to the adapter.

4. The adapter should know that this completion is for hypervisor since it just
store the transaction ID it sends before. After that the adapter explicit notifies
the hypervisor and hypervisor should have enough information to continue the execution.

# Core mechanisms

## DW BE handling
DW BE is two fields of TLP header which called 1st DW byte enable and last DW byte enable. We
know the least access element of memory region enforced by protocol is DW. These two BE fields
enable us to do sub-DW access and each bit of them corresponds to one byte of the starting or
ending DW. The PCIe transcation layer protocol explicit allow non-continously DW access under
certain condition.

## DMA
Should have the ability to access the whole guest memory. I think virtio's implemetation uses
DMA extensibly.

## BAR allocation

Hypervisors usually provide different interface with PCIe root complex. For a typical PCIe
device, we can assign a BAR to it by writing its BAR register. However, that is a mechanism
for host software and PCIe device to negotiate the consumed MMIO or PIO address space. The
region type and size of a BAR is hard coded into the device and is a part of the device core
logic. The adapter do not known anything about the properties of its BAR when hypervisor
request the adapter to allocate its BAR. Hence, we should:

* Probe the BAR as host software usually does.
* Allocate the resource from the system.
* For non-prefetchable BAR region, we simply treat them as MMIO region
* For prefetchable BAR region, we may make them shared memory between guest and simulate device

For totally blockbox like PCIe simulated device, we should detect the BAR by setting and reading
the BAR register. In fact, I think the biggest difference of the two processes is that the simulated
PCIe device only support the standard PCIe BAR reprogramming procedural since it should never be
programmed by the hypervisor preset or the firmware.

That should be very similar of VFIO passthrough. After reading the code, I think CH uses some
kind of shadow config space thecnology. And that not suit for our use case since I don't need this
kind of stuff.

How does BAR reprogramming works in CH?

Every write of configuration space should trigger a reprogramming BAR check by calling the detect_
bar_reprogramming callback of the device. So basically we should implement this callback to support
BAR reprogramming.

## BAR acccessibility

In our simulation, the device model is conceptually a blackbox and do not know
anything about outside. This is suitable to some cases but not all of them. For
example, there are device models such as graphics cards which embedded a DDR chip
inside the device and export certain (configurable) continuous region of it to a
BAR. That simply requries us provides a mechanism to share a memory region between
device model and hypervisor which bypass the transaction sending and receiving
procudure.

Thus we should provides the following mechanism to allow hypervisor to access the
BAR region of the simulated device:

* MMIO based register bank region access. This seems to be fairly simple since we can
  treat the whole BAR region as MMIO registers and trigger PCIe memory write transactions
  to get the result.
* Shared memory based access. We can shared a fixed size memory region bettwen the
  hypervisor and device model and bypass the transaction simulation system. We can
  further provide the ability to change the guest physical address this region mapped
  in the hypervisor by moving the memory slot registered in the KVM virtual machine.
*/

mod adapter;
mod device;
// mod parser;

pub use adapter::{MmioRegion, PciAdapter, PciLane};
pub use device::{PciSimDevice, PciTestDevice};

use log::{debug, error};
use std::convert::TryFrom;

use pci::{
    PciBarConfiguration, PciBarPrefetchable, PciBarRegionType, PciClassCode, PciConfiguration,
    PciDevice, PciDeviceError, PciHeaderType, PciMassStorageSubclass,
};
use vm_device::BusDevice;
use vm_memory::Address;

use vm_allocator::SystemAllocator;
use vm_memory::{GuestAddress, GuestUsize};

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

/// Packet specific data of 32bit memory PCIe transactions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryExtra {
    requester: u16,
    tag: u8,
    addr: u32,
}

/// Packet psecific data of 64bit memory PCIe transactions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Memory64Extra {
    requester: u16,
    tag: u8,
    addr: u64,
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
    MemoryRead(MemoryExtra),
    MemoryRead64(Memory64Extra),
    MemoryReadLock,
    MemoryReadLock64,
    MemoryWrite(MemoryExtra),
    MemoryWrite64(Memory64Extra),
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

    pub fn memory_read(extra: MemoryExtra) -> Self {
        Self::with_type(PacketType::MemoryRead(extra))
    }

    pub fn memory_read64(extra: Memory64Extra) -> Self {
        Self::with_type(PacketType::MemoryRead64(extra))
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
        let len = data.len();
        self.0.data = Some(data);
        self.length(len as u16)
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

        // DW BE rule check
        if (header.length == 1 && header.byte_enable & 0xf == 0)
            | (header.length == 1 && header.byte_enable & 0xf0 != 0)
            | (header.length > 1 && header.byte_enable & 0xf0 == 0)
        {
            return false;
        }

        match header._type {
            Config0Read(extra) | Config0Write(extra) | Config1Read(extra) | Config1Write(extra) => {
                // PCIe 3.0 specification 2.2.7
                if header.trafic_class != TrafficClass::TC0
                    || header.no_snoop
                    || header.relax_ordering
                    || header.length != 0b00001
                {
                    return false;
                }
            }
            _ => unimplemented!(),
        }

        true
    }
}
