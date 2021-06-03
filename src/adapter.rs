use crate::*;

use crossbeam_channel::{select, unbounded, Receiver, Sender};
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread::JoinHandle;

/// The representation of PCIe lane in this library. Basically a full-duplex stream of PCIe transactions.
#[derive(Clone)]
pub struct PciLane {
    pub tx: Sender<Tlp>,
    pub rx: Receiver<Tlp>,
}

impl PciLane {
    fn pair() -> (PciLane, PciLane) {
        let (c1, c2) = (unbounded(), unbounded());

        (
            PciLane { tx: c1.0, rx: c2.1 },
            PciLane { tx: c2.0, rx: c1.1 },
        )
    }
}

#[derive(Debug)]
struct ConfigData {
    reg_idx: usize,
    offset: u64,
    len: usize,
    data: u32,
}

/// The message type between the PciRunnder thread and PciAdapter thread.
#[derive(Debug)]
enum AdapterMessage {
    IoRead(u32, Sender<u32>),
    IoWrite(u32, u8, Sender<()>),
    MemoryRead(u64, usize, Sender<Vec<u8>>),
    MemoryWrite(u64, Vec<u8>, Sender<()>),
    ConfigRead(usize, Sender<u32>),
    ConfigWrite(ConfigData, Sender<()>),
    Exit,
}

/// The reaction carried by a received AdapterMessage
#[derive(Debug)]
enum Reaction {
    /// No action requiered
    Notify(Sender<()>),
    ReadConfig(Sender<u32>),
    Io(Sender<u8>),
    ReadMemory(Sender<Vec<u8>>),
}

fn make_bdf(bus: u8, device: u8, function: u8) -> u16 {
    ((bus as u16) << 8) | ((function as u16 & 0b111) | ((device as u16) << 5))
}

/// The bridge between the adapter and simulated PCIe device.
struct PciSimBridge {
    cmd_rx: Receiver<AdapterMessage>,
    lane: PciLane,
    bdf: u16,
    config_tag: u8,
    store: HashMap<u32, Reaction>,
    handle: JoinHandle<()>,
}

impl PciSimBridge {
    pub fn run(&mut self) {
        loop {
            select! {
                recv(self.cmd_rx) -> msg => {
                    let msg = msg.unwrap();
                    if let AdapterMessage::Exit = msg {
                        break;
                    }
                    self.handle_adapter_msg(msg);
                },

                recv(self.lane.rx) -> msg => {
                    let msg = msg.unwrap();
                    self.handle_transaction_msg(msg);
                }
            }
        }
    }

    fn next_config_tag(&mut self) -> u8 {
        let tag = self.config_tag;
        self.config_tag += 1;
        tag
    }

    fn next_transaction_id(&mut self) -> u32 {
        self.next_config_tag() as u32 | ((self.bdf as u32) << 16)
    }

    fn handle_adapter_msg(&mut self, msg: AdapterMessage) {
        use AdapterMessage::*;
        match msg {
            ConfigRead(idx, sender) => {
                let trans_id = self.next_transaction_id();
                self.store.insert(trans_id, Reaction::ReadConfig(sender));

                let tlp = TlpBuilder::config0_read(ConfigExtra {
                    requester: self.bdf,
                    completer: make_bdf(0x0, 0x3, 0x0),
                    tag: (trans_id & 0xff) as u8,
                    reg: idx as u16,
                })
                .build();

                self.lane.tx.send(tlp).unwrap();
            }
            ConfigWrite(data, sender) => {
                let trans_id = self.next_transaction_id();
                self.store.insert(trans_id, Reaction::Notify(sender));

                let byte_enable = (!(u8::MAX << data.len)) << data.offset;
                let value = data.data << (data.offset * 8);

                let tlp = TlpBuilder::config0_write(ConfigExtra {
                    requester: self.bdf,
                    completer: make_bdf(0x0, 0x3, 0x0),
                    tag: (trans_id & 0xff) as u8,
                    reg: data.reg_idx as u16,
                })
                .byte_enable(byte_enable)
                .data(vec![value])
                .build();

                self.lane.tx.send(tlp).unwrap();
            }
            MemoryRead(addr, size, sender) => {
                let trans_id = self.next_transaction_id();
                self.store.insert(trans_id, Reaction::ReadMemory(sender));

                // TODO: handle memory read request larger than 1024 DW.
                // We do 64 bit memory read transaction anyway.
                // TODO: that's faulty implementation since PCIe spec explicit stated
                // that memory transaction under 4GB boundary should use 32bit packet
                // format. Let's fix this in the future.
                let bits = (addr & 0b11) as u8;
                let byte_enable = (0xff << bits) & 0xf;
                let size = (size + 3) >> 2; // in DW

                let tlp = TlpBuilder::memory_read64(Memory64Extra {
                    requester: self.bdf,
                    tag: (trans_id & 0xff) as u8,
                    addr,
                })
                .byte_enable(byte_enable)
                .length(size as u16)
                .build();

                self.lane.tx.send(tlp).unwrap();
            }
            _ => unimplemented!(),
        }
    }

    fn handle_transaction_msg(&mut self, msg: Tlp) {
        match msg.header._type {
            PacketType::CompletionData(extra) => {
                if let Some(reaction) = self.store.get(&msg.header.transaction_id()) {
                    match reaction {
                        Reaction::ReadConfig(sender) => {
                            sender.send(msg.data.unwrap()[0]).unwrap();
                        }
                        Reaction::Notify(sender) => {
                            sender.send(()).unwrap();
                        }
                        Reaction::ReadMemory(sender) => {
                            // TODO: optimize the logic to handle non-continuously QW aligned access.
                            let dw = msg.data.unwrap();
                            let dw_size = dw.len();
                            let offset = (extra.lower_address & 0b11) as usize;
                            let first_dw = dw[0].to_be_bytes();
                            let mut data = Vec::from(&first_dw[offset..4]);
                            if dw_size > 1 {
                                for i in 1..dw_size {
                                    let offset = if i == dw_size - 1 {
                                        4 - (msg.header.byte_enable & 0xf0 | 0x8).leading_zeros()
                                            as usize
                                    } else {
                                        4
                                    };
                                    data.extend_from_slice(&dw[i].to_be_bytes()[0..offset]);
                                }
                            }

                            sender.send(data).unwrap();
                        }
                        _ => unimplemented!(),
                    }
                }
            }
            _ => unimplemented!(),
        }
    }
}

#[derive(Copy, Clone)]
pub struct MmioRegion {
    pub start: GuestAddress,
    pub length: GuestUsize,
    pub type_: PciBarRegionType,
    pub bar_reg: usize,
    pub slot_mapped: bool,
    pub mem_slot: Option<u32>,
    pub host_addr: Option<u64>,
    pub mmap_size: Option<usize>,
}

/// The adapter PCI device exporting an hypervisor friendly interface.
pub struct PciAdapter {
    tx: Sender<AdapterMessage>,
    pub(crate) mmio_regions: Vec<MmioRegion>,
    handle: JoinHandle<()>,
}

impl PciAdapter {
    /// Request the runner thread to send a type 0 config read transaction to the simulated device.
    /// Then block and wait for the completion transaction.
    pub fn config_read(&self, reg_idx: usize) -> u32 {
        let (tx, rx) = unbounded();
        self.tx
            .send(AdapterMessage::ConfigRead(reg_idx, tx))
            .unwrap();
        rx.recv().unwrap()
    }

    /// Request the runner thread to send a type 0 config write transaction to the simulated device.
    /// Then block and wait for the completion transaction.
    pub fn config_write(&self, reg_idx: usize, offset: u64, data: &[u8]) {
        let (tx, rx) = unbounded();
        let len = data.len();
        let mut bytes = 0;

        for b in data.iter().rev() {
            bytes = (bytes << 8) | *b as u32;
        }

        let data = ConfigData {
            reg_idx,
            offset,
            len,
            data: bytes,
        };
        self.tx.send(AdapterMessage::ConfigWrite(data, tx)).unwrap();
        rx.recv().unwrap()
    }

    pub fn bar_mmio_read(&self, addr: u64, data: &mut [u8]) {
        if let Some(region) = self.find_region(addr) {
            if data.len() > 8 {
                error!("Invalid access to MMIO region {:#x} {}", addr, data.len());
                data.fill(0xff);
                return;
            }

            if region.slot_mapped {
                error!(
                    "Region should be memory backed, maybe you forget to register the slot? {:#x}",
                    addr
                );
            }

            let (tx, rx) = unbounded();
            self.tx
                .send(AdapterMessage::MemoryRead(addr, data.len(), tx))
                .unwrap();
            let value = rx.recv().unwrap();
            assert_eq!(value.len(), data.len());
            data.copy_from_slice(&value);
        } else {
            error!("Invalid access to unknown BAR region {:#x}", addr);
        }
    }

    pub fn bar_write() {
        unimplemented!();
    }

    fn config_write_u32(&self, reg_idx: usize, data: u32) {
        self.config_write(reg_idx, 0, &data.to_le_bytes());
    }

    /// Helper function to return the result when we write all 1s to a BAR. The original value of
    /// the BAR is restored after this detection.
    fn detect_bar(&mut self, reg_idx: usize) -> u32 {
        let pre = self.read_config_register(reg_idx);
        self.config_write_u32(reg_idx, u32::MAX);
        let ret = self.read_config_register(reg_idx);
        self.config_write_u32(reg_idx, pre);
        ret
    }

    /// Find a registered BAR region which contains the given guest physical address
    fn find_region(&self, addr: u64) -> Option<MmioRegion> {
        for region in self.mmio_regions.iter() {
            if addr >= region.start.raw_value()
                && addr < region.start.unchecked_add(region.length).raw_value()
            {
                return Some(*region);
            }
        }
        None
    }

    /// Scan all of the six BAR and execute the callback for them.
    pub fn scan_bar(&mut self) -> Vec<MmioRegion> {
        use PciBarRegionType::*;

        let mut regions = vec![];
        let mut bar_reg = BAR0_REG;

        while bar_reg < BAR0_REG + NUM_BAR_REGS {
            let lsb_size: u32 = self.detect_bar(bar_reg);
            let region_size: u64;
            let mut slot_mapped = false;
            let mut is_64bit = false;

            if lsb_size == 0 {
                bar_reg += 1;
                continue;
            }

            let region_type = if lsb_size & 0x1 == 1 {
                IoRegion
            } else if (lsb_size >> 1) & 0x3 == 0x2 {
                Memory64BitRegion
            } else {
                Memory32BitRegion
            };

            let prefetchable = lsb_size & 0b1000 != 0;

            match region_type {
                Memory64BitRegion => {
                    let msb_size = self.detect_bar(bar_reg + 1);
                    region_size =
                        !(((msb_size as u64) << 32) | (lsb_size as u64 & 0xffff_fff0)) + 1;
                    slot_mapped = prefetchable;
                    is_64bit = true;
                }
                Memory32BitRegion => {
                    region_size = !(lsb_size as u64 & 0xffff_fff0) + 1;
                    slot_mapped = prefetchable;
                }
                IoRegion => {
                    region_size = (!(lsb_size & 0xffff_fffc) + 1) as u64;
                }
            }

            regions.push(MmioRegion {
                start: GuestAddress(0),
                length: region_size,
                type_: region_type,
                bar_reg,
                mem_slot: None,
                host_addr: None,
                mmap_size: None,
                slot_mapped,
            });

            bar_reg += if is_64bit { 2 } else { 1 };
        }

        regions
    }

    pub fn join(self) {
        self.handle.join().unwrap();
    }

    pub fn stop(&self) {
        self.tx.send(AdapterMessage::Exit).unwrap();
    }

    pub fn start(mut device: Box<dyn PciSimDevice + Send + Sync>) -> PciAdapter {
        let (lane, device_lane) = PciLane::pair();
        let (tx, cmd_rx) = unbounded();
        let handle = std::thread::spawn(move || device.as_mut().run(&device_lane));
        let mut runner = PciSimBridge {
            handle,
            lane,
            cmd_rx,
            config_tag: 0,
            store: HashMap::new(),
            bdf: make_bdf(0x0, 0x2, 0x0),
        };

        let handle = std::thread::spawn(move || {
            runner.run();
        });

        PciAdapter {
            tx,
            handle,
            mmio_regions: vec![],
        }
    }
}

const BAR0_REG: usize = 4;
const NUM_BAR_REGS: usize = 6;

impl PciDevice for PciAdapter {
    fn write_config_register(
        &mut self,
        reg_idx: usize,
        offset: u64,
        data: &[u8],
    ) -> Option<Arc<Barrier>> {
        self.config_write(reg_idx, offset, data);
        None
    }

    fn read_config_register(&mut self, reg_idx: usize) -> u32 {
        self.config_read(reg_idx)
    }

    fn allocate_bars(
        &mut self,
        allocator: &mut SystemAllocator,
    ) -> std::result::Result<Vec<(GuestAddress, GuestUsize, PciBarRegionType)>, PciDeviceError>
    {
        use PciBarRegionType::*;

        let mut ranges = vec![];
        let mut regions = self.scan_bar();
        self.mmio_regions.clear();

        for region in regions.iter_mut() {
            match region.type_ {
                Memory64BitRegion => {
                    region.start = allocator
                        .allocate_mmio_addresses(None, region.length, Some(0x10))
                        .ok_or(PciDeviceError::IoAllocationFailed(region.length))?;
                    self.config_write_u32(
                        region.bar_reg + 1,
                        (region.start.raw_value() >> 32) as u32,
                    );
                    self.config_write_u32(region.bar_reg, region.start.raw_value() as u32);
                }
                Memory32BitRegion => {
                    region.start = allocator
                        .allocate_mmio_hole_addresses(None, region.length, Some(0x10))
                        .ok_or(PciDeviceError::IoAllocationFailed(region.length))?;
                    self.config_write_u32(region.bar_reg, region.start.raw_value() as u32);
                }
                IoRegion => {
                    region.start = allocator
                        .allocate_io_addresses(None, region.length, Some(0x4))
                        .ok_or(PciDeviceError::IoAllocationFailed(region.length))?;
                    debug!("write io addr {:#x}", region.start.raw_value());
                    self.config_write_u32(region.bar_reg, region.start.raw_value() as u32);
                }
            }

            debug!(
                "allocate BAR reg{}; address: {:#x}; region_type: {}",
                region.bar_reg,
                region.start.raw_value(),
                region.type_ as u8
            );

            ranges.push((region.start, region.length, region.type_));
            self.mmio_regions.push(*region);
        }

        Ok(ranges)
    }

    fn free_bars(
        &mut self,
        allocator: &mut SystemAllocator,
    ) -> std::result::Result<(), PciDeviceError> {
        for region in self.mmio_regions.iter() {
            match region.type_ {
                PciBarRegionType::IoRegion => {
                    #[cfg(target_arch = "x86_64")]
                    allocator.free_io_addresses(region.start, region.length);
                    #[cfg(target_arch = "aarch64")]
                    error!("I/O region is not supported");
                }
                PciBarRegionType::Memory32BitRegion => {
                    allocator.free_mmio_hole_addresses(region.start, region.length);
                }
                PciBarRegionType::Memory64BitRegion => {
                    allocator.free_mmio_addresses(region.start, region.length);
                }
            }
        }
        Ok(())
    }

    fn read_bar(&mut self, base: u64, offset: u64, data: &mut [u8]) {
        self.bar_mmio_read(base + offset, data);
    }

    fn write_bar(&mut self, base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        None
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

impl BusDevice for PciAdapter {
    fn read(&mut self, base: u64, offset: u64, data: &mut [u8]) {
        self.read_bar(base, offset, data)
    }

    fn write(&mut self, base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        self.write_bar(base, offset, data)
    }
}
