// The basic idea is to handle some common PCIe transaction with common code.
// And Makes us easy to implement an architecture where

// We should separate the bridge between specific hypervisor and the device
// implementation. Make it easy to port the device between different rust-vmm
// hypervisor.

/// Basic idea of components:
///     Bridge: the bridge to translate the access from the guest into PCIe
///             transaction.
///     SimDevice: the entity to handle PCIE transaction and I think it should
///             be the wrapper of the C model. Maybe I can write some glue code
///             to combine some RTL device into the code model.
// after reading some documentation. It seems that the top layer, i.e. the software
// layer or the device core handles TLP HDR and Data directly.
// So my abstraction should working with the PCIe TLP layer.

// When the CPU issue a memory operation of a address managed by PCIE root complex.
// The PCIE root complex issue corresponding transaction to the destination device.
// When it receives the corresponding completion. Then the CPU get the response and
// continue the execution.

// So when KVM get an MMIO_EXIT or PIO_EXIT exit and start to process the register level
// access. We should send the transcation and wait for the completion. The vcpu thread
// should wait for the completion. And we should provide the device received the request
// a common mechanism to notify the completion.
// Solution: oneshot channel

// Multi-lane PCIE device support. We should consider the situation where most graphics
// card is a high performance device with multiple PCIE lanes.
// Solution: Flexible transaction queue architecture.

// let's think about the threading model
//
// most case:
// vcpu thread => trigger MMIO exit => post a transaction to the queue and blocking
//
// device thread => dequeue a transaction and handle it => trigger
//          PCIE io transaction : read and write the registers
//          Memory transaction: read and write the registers
//          Message transaction: handle it
//          Interrupt transaction:
use crate::*;

// another architecture after reading more documentation
// Adapter Device -> Simulation Device
// Maybe some pipeline based architecture

// The simulation device provides a basic interface:
//      * transaction queue
//      * transaction dequeue

// Basically the adapter device queue the transaction and wait for the response
// There might be another layer?
//
// Adapter Device -> Queue Transaction -> Blocking and wait -> Receive the completion -> Continue to execute
//
// No matter what, the Simulation device should have a thread safe interface. So that the simulation device can
// have several simulation threads. The best interface I think should be a

// The immediate layer should have two hash table. When we queue a transaction into the Simulation Device
// We put the transaction into the hash table. When we get a completion, we just look into the hash table and
// search the request transaction and made the completion. The whole system should be an event drive system.

// So basically we should implement the simulation device first and complete the infrastructures.
// Build up the test simulation framework.

pub trait PciSimDevice {
    fn run(&mut self, lane: &PciLane);
}
struct PciLoopback {}

impl PciLoopback {
    pub fn new() -> Self {
        Self {}
    }
}

impl PciSimDevice for PciLoopback {
    fn run(&mut self, lane: &PciLane) {
        while let Ok(trans) = lane.rx.recv() {
            match trans.header._type {
                PacketType::Config0Read(extra) => {}

                _ => unimplemented!(),
            }
            lane.tx.send(trans).unwrap();
        }
    }
}

/// The simplest representation of a virtual PCIe lane. It is basically a simple full-duplex
/// channel allowing the adapter and the device to communicate through PCIe transactions.
// Another thought:
// PciSimAdapter {  TX RX channel of the PCIE transaction itself  } & handle the transaction logic
// PciSimDevice: provides a run method and methods to create the channel

// When PciSimAdapter initialized, it consume a PciSimDevice and launch another thread. Then run the
// run() method inside this thread. In fact we do not need any information about the PciSimDevice and
// want it run inside its dedicated thread.

// Common Part of PCIE device:
//      There are huge common behavior to all PCIe devices since they comform to the same standard.
//      We should provide a common behavior model to react to certain PCIe transaction.
//      We should even make the common behavior configurable as a template for easy bring up a basic
//      PCIe device.
use pci::{PciClassCode, PciConfiguration, PciHeaderType, PciMassStorageSubclass};

/// Shared common behaviour of a classic PCIe device. Users of this library should delegate the common
/// bahaviour handling such as IO, Config Space, MMIO transaction to it.
pub struct PciDeviceCommon {
    config: PciConfiguration,
}

impl PciDeviceCommon {
    fn new() -> PciDeviceCommon {
        let config = PciConfiguration::new(
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

        PciDeviceCommon { config }
    }
}

impl PciSimDevice for PciDeviceCommon {
    fn run(&mut self, lane: &PciLane) {
        use PacketType::*;

        while let Ok(trans) = lane.rx.recv() {
            match trans.header._type {
                PacketType::IoRead => {
                    let h = self.config.read_config_register(0);
                    println!("{:#x}", h);
                }

                PacketType::IoWrite => {}

                PacketType::Config0Read(extra) => {
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
                    self.config.write_config_register(
                        extra.reg as usize,
                        0,
                        value.to_le_bytes().as_ref(),
                    );

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

                _ => unimplemented!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn common() {
        let device = PciDeviceCommon::new();
        let adapter = PciAdapter::start(Box::new(device));
        let v = adapter.config_read(0x0);
        assert_eq!(v, 0x56781234);

        for i in 0..64 {
            let v = adapter.config_read(i);
            println!("{} {:#x}", i, v);
        }

        adapter.config_write(0x0, 0x55556666);
        let v = adapter.config_read(0x0);
        println!("vendor is {:#x}", v);
        adapter.stop();
        adapter.join();
    }
}
