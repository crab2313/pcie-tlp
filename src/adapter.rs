use crate::*;
use crossbeam_channel::{select, unbounded, Receiver, Sender};
use pci::PciDevice;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread::JoinHandle;
use vm_device::BusDevice;

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

// Thought:
// How does VCPU thread access the device?

// When VCPU try to access the PCI config space
//          The hypervisor typically generate a MMIO or IO EXIT
//          No matter what, the VCPU thread simply exit from KVM, i.e stop blocking
//          on kvm_cpu_run, and start to process the PIO/MMIO access.
//          Basically. We should send a PCIE transaction to the device core and
//          wait for the completion transaction back.

//          That means the VCPU thread should still block and wait for the completion.
//          So it should block on something, like a event notification.
//          It requires that the Adapter itself have the ability to process the event
//          through some message passing system.

//          So the adapter should provide some interface like that
//
//              Adapter.read_config
//              Adapter.write_config
//              Adapter.read_memory
//          For the completion transaction.
//          These interface should be called in another thread.

//          So the adapter itself should have some logic to handle PCIE transaction
//          in a dedicated thread. like

//          Adapter thread busy processing messages
//              Config Request(Read | Write)

#[derive(Debug)]
struct ConfigData {
    reg_idx: usize,
    offset: u64,
    len: usize,
    data: u32,
}

#[derive(Debug)]
enum AdapterMessage {
    IoRead(usize, Sender<u32>),
    IoWrite,
    MemoryRead,
    MemoryWrite,
    ConfigRead(usize, Sender<u32>),
    ConfigWrite(ConfigData, Sender<()>),
    Exit,
}

#[derive(Debug)]
enum Reaction {
    /// No action requiered
    Notify(Sender<()>),
    ReadConfig(Sender<u32>),
    Io(Sender<u8>),
}
/// The adapter PCI device exporting an hypervisor friendly interface.
#[derive(Debug)]
pub struct PciAdapter {
    tx: Sender<AdapterMessage>,
    handle: JoinHandle<()>,
}

fn make_bdf(bus: u8, device: u8, function: u8) -> u16 {
    ((bus as u16) << 8) | ((function as u16 & 0b111) | ((device as u16) << 5))
}

/// The bridge between the adapter and simulated PCIe device.
struct PciRunner {
    cmd_rx: Receiver<AdapterMessage>,
    lane: PciLane,
    bdf: u16,
    config_tag: u8,
    store: HashMap<u32, Reaction>,
    handle: JoinHandle<()>,
}

impl PciRunner {
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
                        _ => unimplemented!(),
                    }
                }
            }
            _ => unimplemented!(),
        }
    }
}

impl PciAdapter {
    /// Ask the runner thread sending a config read transaction to the simulated device.
    /// And block waiting for the completion transaction.
    pub fn config_read(&self, reg_idx: usize) -> u32 {
        let (tx, rx) = unbounded();
        self.tx
            .send(AdapterMessage::ConfigRead(reg_idx, tx))
            .unwrap();
        rx.recv().unwrap()
    }

    pub fn config_write(&self, reg_idx: usize, offset: u64, data: &[u8]) {
        let (tx, rx) = unbounded();
        let len = data.len();
        let mut bytes = 0;

        for b in data {
            bytes = (bytes << 8) & *b as u32;
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

    pub fn join(self) {
        self.handle.join().unwrap();
    }

    pub fn stop(&self) {
        self.tx.send(AdapterMessage::Exit).unwrap();
    }
}

impl PciAdapter {
    pub fn start(mut device: Box<dyn PciSimDevice + Send + Sync>) -> PciAdapter {
        let (lane, device_lane) = PciLane::pair();
        let (tx, cmd_rx) = unbounded();
        let handle = std::thread::spawn(move || device.as_mut().run(&device_lane));
        let mut runner = PciRunner {
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

        PciAdapter { tx, handle }
    }
}

impl BusDevice for PciAdapter {}

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

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}
