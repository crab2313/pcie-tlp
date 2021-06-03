// We should send the transcation and wait for the completion when KVM get an MMIO_EXIT or
// PIO_EXIT exit and start to process the register level access. And we should also provide
// the device which receives the request a common mechanism to notify the completion.

// Multi-lane PCIe device support. We should consider the situation where most graphics
// card are high performance device with multiple PCIe lanes.
// Solution: Flexible transaction queue architecture.

use crate::*;

/// The simulated PCIe transaction layer device model.
///
/// The device model simply receives PCIe transactions and handle them conform to PCIe specification.
/// When [`PciAdapter`] is initialized, it consume a PciSimDevice and launch another thread. Then run the
/// [`PciSimDevice::run`] method inside this thread. In fact, we don't need any information about the
/// [`PciSimDevice`] and  want it run inside its dedicated thread.
pub trait PciSimDevice {
    /// Thread callback of simulated device model.
    ///
    /// * `lane` - full-duplexed PCIe lane to communicate with bridge thread.
    fn run(&mut self, lane: &PciLane);
}

/// A simple PCIe transaction level simulated device for test purpose.
pub struct PciTestDevice {
    config: PciConfiguration,
}

impl PciTestDevice {
    pub fn new() -> PciTestDevice {
        let mut config = PciConfiguration::new(
            0x1234,
            0x5678,
            0x0001,
            PciClassCode::Other,
            &PciMassStorageSubclass::MassStorage,
            None,
            PciHeaderType::Device,
            0x5555,
            0x6666,
            None,
        );

        let bar = PciBarConfiguration::new(
            0,
            0x100000,
            PciBarRegionType::Memory64BitRegion,
            PciBarPrefetchable::NotPrefetchable,
        );

        config.add_pci_bar(&bar).unwrap();

        let bar = PciBarConfiguration::new(
            2,
            0x100,
            PciBarRegionType::IoRegion,
            PciBarPrefetchable::NotPrefetchable,
        );

        config.add_pci_bar(&bar).unwrap();

        PciTestDevice { config }
    }
}

impl PciSimDevice for PciTestDevice {
    fn run(&mut self, lane: &PciLane) {
        use PacketType::*;

        while let Ok(trans) = lane.rx.recv() {
            match trans.header._type {
                IoRead => {
                    let h = self.config.read_config_register(0);
                    println!("{:#x}", h);
                }

                IoWrite => {}

                Config0Read(extra) => {
                    let value = self.config.read_config_register(extra.reg as usize);

                    let tlp = TlpBuilder::completion_data(CompletionExtra {
                        requester: extra.requester,
                        completer: extra.completer,
                        tag: extra.tag,
                        bcm: false,
                        byte_count: 4,
                        status: 0,
                        lower_address: 0,
                    })
                    .data(vec![value])
                    .build();

                    lane.tx.send(tlp).unwrap();
                }

                Config0Write(extra) => {
                    let value = trans.data.unwrap()[0];
                    let be = trans.header.byte_enable;
                    let offset = be.trailing_zeros() as u64;
                    let len = (8 - be.leading_zeros() - offset as u32) as usize;
                    let data = &u32::to_le_bytes(value >> offset)[0..len];

                    self.config
                        .write_config_register(extra.reg as usize, offset, data);

                    let tlp = TlpBuilder::completion_data(CompletionExtra {
                        requester: extra.requester,
                        completer: extra.completer,
                        tag: extra.tag,
                        bcm: false,
                        byte_count: 4,
                        status: 0,
                        lower_address: 0,
                    })
                    .data(vec![value])
                    .build();

                    lane.tx.send(tlp).unwrap();
                }

                // Ignore type 1 configuration transaction since it is for PCI bridge
                Config1Read(_) | Config1Write(_) => (),

                MemoryRead64(extra) => {
                    let lower_address = (extra.addr as u8 & 0b1111100)
                        | ((trans.header.byte_enable & 0xf).trailing_zeros() as u8 % 4);

                    let byte_enable = if trans.header.length == 1 { 0x0f } else { 0xff };

                    let tlp = TlpBuilder::completion_data(CompletionExtra {
                        requester: extra.requester,
                        completer: 0,
                        tag: extra.tag,
                        bcm: false,
                        byte_count: 0,
                        status: 0,
                        lower_address,
                    })
                    .byte_enable(byte_enable)
                    .data(vec![0x12345678; trans.header.length as usize])
                    .build();

                    lane.tx.send(tlp).unwrap();
                }
                _ => unimplemented!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pci::PciDevice;

    use super::*;

    #[test]
    fn common() {
        let device = PciTestDevice::new();
        let adapter = PciAdapter::start(Box::new(device));

        adapter.config_write(0x0, 0, &u32::to_le_bytes(0x11112222));
        assert_eq!(adapter.config_read(0), 0x56781234);

        adapter.stop();
        adapter.join();
    }

    #[test]
    fn bar() {
        let device = PciTestDevice::new();
        let mut adapter = PciAdapter::start(Box::new(device));

        adapter.write_config_register(4, 0, &(0xffffffffu32).to_le_bytes());
        adapter.write_config_register(5, 0, &(0xffffffffu32).to_le_bytes());
        adapter.write_config_register(6, 0, &(0xff00u32).to_le_bytes());

        assert_eq!(adapter.read_config_register(4), 0xfff0_0004);
        assert_eq!(adapter.read_config_register(5), 0xffff_ffff);
        assert_eq!(adapter.read_config_register(6), 0xff01);

        adapter.write_config_register(4, 0, &(0x7000_0000u32).to_be_bytes());
        adapter.write_config_register(5, 0, &(0x0000_0001u32).to_be_bytes());

        adapter.mmio_regions.push(MmioRegion {
            start: GuestAddress(0x1_7000_0000),
            length: 0x100000,
            type_: PciBarRegionType::Memory64BitRegion,
            bar_reg: 0,
            mem_slot: None,
            host_addr: None,
            mmap_size: None,
            slot_mapped: false,
        });

        let mut data = [0u8; 4];
        adapter.bar_mmio_read(0x1_7000_0000, &mut data);
        assert_eq!(data, [0x12, 0x34, 0x56, 0x78]);

        let mut data = [0u8; 8];
        adapter.bar_mmio_read(0x1_7000_0000, &mut data);
        assert_eq!(data, [0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78]);

        for i in 0..64 {
            let v = adapter.config_read(i);
            println!("{} {:#x}", i, v);
        }

        adapter.stop();
        adapter.join();
    }
}
