This crate is one of my attempt to implement a infrastructure aiming to be a bridge
between transaction-level simulated PCIe device and hypervisor. In fact, common
hypervisor's implementation of PCIe devices are simply some kind of register-level
simulation. We need a bridge between transaction-level simulated PCIe device and
hypervisor since this kind of simulated device only speaks PCIe transaction packet.