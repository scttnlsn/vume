//! Integration test for the full VM lifecycle.
//!
//! Requires root (sudo) for networking and Firecracker.

use vume::config;
use vume::ssh::wait_for_ready;
use vume::state::{StateManager, VmStatus};
use vume::vm::VM;

fn init_config() {
    let cfg = config::Config::load().expect("failed to load config");
    config::init(cfg);
}

#[test]
fn test_vm_lifecycle() {
    init_config();

    // Create and start a new VM
    let mut vm = VM::new(None).expect("failed to create VM");
    let vm_id = vm.id().to_string();

    let info = vm.start().expect("failed to start VM");
    assert_eq!(info.status, VmStatus::Running);
    assert!(info.ip.starts_with("172.16.0."));
    assert!(info.pid > 0);

    assert!(wait_for_ready(&info.ip, 30), "SSH not ready after start");

    // Stop the VM
    VM::stop(&vm_id).expect("failed to stop VM");

    {
        let state = StateManager::new().expect("failed to open state");
        let info = state
            .get_vm(&vm_id)
            .expect("failed to query VM")
            .expect("VM not found after stop");
        assert_eq!(info.status, VmStatus::Stopped);
    }

    // Resume the stopped VM
    let mut resumed = VM::new(Some(&vm_id)).expect("failed to create resumed VM");
    let info = resumed.start().expect("failed to resume VM");
    assert_eq!(info.status, VmStatus::Running);

    assert!(wait_for_ready(&info.ip, 30), "SSH not ready after resume");

    // Destroy the VM
    VM::destroy(&vm_id).expect("failed to destroy VM");

    {
        let state = StateManager::new().expect("failed to open state");
        let info = state.get_vm(&vm_id).expect("failed to query VM");
        assert!(info.is_none(), "VM should be gone after destroy");
    }
}
