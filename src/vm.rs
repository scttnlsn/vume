use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use log::info;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use uuid::Uuid;

use crate::config::get;
use crate::firecracker::Firecracker;
use crate::network::NetworkManager;
use crate::state::{StateManager, VMInfo, VmStatus};

#[derive(Debug)]
pub struct VM {
    id: String,
    kernel: PathBuf,
    vm_dir: PathBuf,
    rootfs: PathBuf,
    socket: PathBuf,

    // Set during boot
    ip: Option<String>,
    tap: Option<String>,
    pid: Option<u32>,
    resuming: bool,
}

impl VM {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn new(vm_id: Option<&str>) -> Result<Self> {
        let cfg = get();
        let id = vm_id.map_or_else(
            || Uuid::new_v4().simple().to_string()[..8].to_string(),
            str::to_string,
        );

        let kernel = fs::canonicalize(&cfg.kernel)
            .with_context(|| format!("Kernel not found at {}", cfg.kernel.display()))?;

        let vm_dir = cfg.vms_dir().join(&id);
        let socket = vm_dir.join("firecracker.sock");
        let rootfs = PathBuf::from(cfg.zvol_path(&id));

        Ok(Self {
            id,
            kernel,
            vm_dir,
            rootfs,
            socket,
            ip: None,
            tap: None,
            pid: None,
            resuming: false,
        })
    }

    /// Start the VM (non-blocking). Returns info about the running VM.
    pub fn start(&mut self) -> Result<VMInfo> {
        let state = StateManager::new()?;
        match self.do_start(&state) {
            Ok(info) => Ok(info),
            Err(e) => {
                self.rollback(&state);
                Err(e)
            }
        }
    }

    fn do_start(&mut self, state: &StateManager) -> Result<VMInfo> {
        let cfg = get();
        let tap = format!("tap-{}", self.id);
        self.tap = Some(tap.clone());

        // Check if this ID belongs to an existing stopped/errored VM
        if let Some(existing) = state.get_vm(&self.id)? {
            match existing.status {
                VmStatus::Running | VmStatus::Booting => {
                    bail!("VM {} is already {}", self.id, existing.status);
                }
                VmStatus::Stopped | VmStatus::Error => {
                    self.resuming = true;
                }
            }
        }

        let info = if self.resuming {
            info!("resuming VM: {}", self.id);
            state.resume_vm(&self.id, &tap)?
        } else {
            info!("preparing new VM: {}", self.id);
            fs::create_dir_all(&self.vm_dir)?;

            let snapshot = resolve_rootfs_snapshot()?;
            run_zfs(&["clone", &snapshot, &cfg.zfs_dataset(&self.id)])?;

            state.reserve_vm(&self.id, &tap)?
        };

        // Setup networking
        self.ip = Some(info.ip.clone());
        NetworkManager::ensure_bridge(cfg.network.outbound_if.as_deref())?;
        NetworkManager::create_tap(&tap)?;

        // Start Firecracker
        let fc = Firecracker::new(&self.socket, &self.kernel, &self.rootfs, &tap, &info.ip)?;
        let pid = fc.launch()?;
        self.pid = Some(pid);
        fc.configure()?;
        fc.start()?;

        state.mark_running(&self.id, i64::from(pid))
    }

    fn rollback(&self, state: &StateManager) {
        if let Some(pid) = self.pid {
            kill_process(pid, true);
        }
        if let Some(ref tap) = self.tap {
            NetworkManager::delete_tap(tap);
        }
        if self.resuming {
            let _ = state.update_status(&self.id, VmStatus::Stopped);
            return;
        }
        let _ = cleanup_vm(&self.id, state);
    }

    /// Stop a VM by ID. Kills the process and removes the tap device.
    pub fn stop(vm_id: &str) -> Result<()> {
        let state = StateManager::new()?;
        let vm = get_vm(&state, vm_id)?;
        teardown(&vm);
        state.update_status(&vm.id, VmStatus::Stopped)?;
        println!("VM {} stopped", vm.id);
        Ok(())
    }

    /// Stop and fully remove a VM (process, tap, files, and state record)
    pub fn destroy(vm_id: &str) -> Result<()> {
        let state = StateManager::new()?;
        let vm = get_vm(&state, vm_id)?;
        teardown(&vm);
        cleanup_vm(&vm.id, &state)?;
        info!("destroyed VM: {}", vm.id);
        Ok(())
    }
}

fn resolve_rootfs_snapshot() -> Result<String> {
    let pool = &get().zfs_pool;
    let output = Command::new("zfs")
        .args([
            "get",
            "-H",
            "-o",
            "value",
            "vume:latest",
            &format!("{}/rootfs", pool),
        ])
        .output()
        .context("Failed to run zfs get")?;

    if !output.status.success() {
        bail!("zfs get failed");
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() || version == "-" {
        bail!("vume:latest property not set on {}/rootfs", pool);
    }
    Ok(format!("{}/rootfs@{}", pool, version))
}

/// Remove a VM's filesystem, ZFS dataset, and state record.
/// Attempts all steps even if individual ones fail, returning the first error.
fn cleanup_vm(vm_id: &str, state: &StateManager) -> Result<()> {
    let cfg = get();
    let vm_dir = cfg.vms_dir().join(vm_id);
    let mut errors = Vec::new();

    if vm_dir.exists() {
        if let Err(e) = fs::remove_dir_all(&vm_dir) {
            errors.push(anyhow::Error::from(e));
        }
    }
    if let Err(e) = run_zfs(&["destroy", "-r", &cfg.zfs_dataset(vm_id)]) {
        errors.push(e);
    }
    if let Err(e) = state.delete_vm(vm_id) {
        errors.push(e);
    }

    match errors.into_iter().next() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn run_zfs(args: &[&str]) -> Result<()> {
    let status = Command::new("zfs")
        .args(args)
        .status()
        .context("Failed to run zfs")?;
    if !status.success() {
        bail!("zfs {} failed", args.join(" "));
    }
    Ok(())
}

fn get_vm(state: &StateManager, vm_id: &str) -> Result<VMInfo> {
    state
        .get_vm(vm_id)?
        .with_context(|| format!("VM {vm_id} not found"))
}

fn teardown(vm: &VMInfo) {
    if let Some(pid) = vm.pid_u32() {
        kill_process(pid, true);
    }
    NetworkManager::delete_tap(&vm.tap);
}

fn kill_process(pid: u32, force: bool) {
    let nix_pid = Pid::from_raw(pid as i32);
    if signal::kill(nix_pid, Signal::SIGTERM).is_ok() {
        for _ in 0..30 {
            if signal::kill(nix_pid, None).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if force {
            let _ = signal::kill(nix_pid, Signal::SIGKILL);
        }
    }
}
