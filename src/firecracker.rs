use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::json;

use crate::network::BRIDGE_IP;

pub struct Firecracker {
    socket: PathBuf,
    client: Client,
    kernel: PathBuf,
    rootfs: PathBuf,
    tap: String,
    ip: String,
    vcpu: u32,
    mem: u32,
}

impl Firecracker {
    pub fn new(socket: &Path, kernel: &Path, rootfs: &Path, tap: &str, ip: &str) -> Result<Self> {
        let socket = if socket.is_absolute() {
            socket.to_path_buf()
        } else {
            env::current_dir()?.join(socket)
        };
        let client = Client::builder()
            .unix_socket(socket.clone())
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            socket,
            client,
            kernel: kernel.to_owned(),
            rootfs: rootfs.to_owned(),
            tap: tap.to_owned(),
            ip: ip.to_owned(),
            vcpu: 2,
            mem: 1024,
        })
    }

    /// Start the Firecracker process (detached). Returns the PID.
    pub fn launch(&self) -> Result<u32> {
        if self.socket.exists() {
            fs::remove_file(&self.socket)?;
        }

        let fc_path = fs::canonicalize("vume/firecracker")
            .context("Firecracker binary not found at vume/firecracker")?;

        let mut child = Command::new(fc_path)
            .arg("--api-sock")
            .arg(&self.socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to start Firecracker")?;

        let pid = child.id();

        // Wait for socket to appear (~5 second timeout)
        for _ in 0..50 {
            if self.socket.exists() {
                return Ok(pid);
            }
            // Check if the process exited early (e.g. bad args, missing libs)
            if let Some(status) = child
                .try_wait()
                .context("Failed to check Firecracker status")?
            {
                bail!("Firecracker exited immediately with {status}");
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!(
            "Firecracker socket did not appear at {}",
            self.socket.display()
        );
    }

    pub fn configure(&self) -> Result<()> {
        self.put(
            "/machine-config",
            &json!({
                "vcpu_count": self.vcpu,
                "mem_size_mib": self.mem,
            }),
        )?;

        self.put(
            "/boot-source",
            &json!({
                "kernel_image_path": self.kernel.to_string_lossy(),
                "boot_args": format!(
                    "console=ttyS0 reboot=k panic=1 pci=off ip={}::{}:255.255.255.0::eth0:off",
                    self.ip, BRIDGE_IP
                ),
            }),
        )?;

        self.put(
            "/drives/rootfs",
            &json!({
                "drive_id": "rootfs",
                "path_on_host": self.rootfs.to_string_lossy(),
                "is_root_device": true,
                "is_read_only": false,
            }),
        )?;

        self.put(
            "/network-interfaces/eth0",
            &json!({
                "iface_id": "eth0",
                "host_dev_name": self.tap,
            }),
        )?;

        Ok(())
    }

    pub fn start(&self) -> Result<()> {
        self.put("/actions", &json!({"action_type": "InstanceStart"}))?;
        Ok(())
    }

    fn put(&self, path: &str, data: &serde_json::Value) -> Result<()> {
        // The domain is ignored when using a unix socket; only the path matters.
        let url = format!("http://localhost{path}");
        let response = self
            .client
            .put(&url)
            .json(data)
            .send()
            .with_context(|| format!("PUT {path} failed"))?;

        let status = response.status();
        if status.is_client_error() || status.is_server_error() {
            let body = response.text().unwrap_or_default();
            log::warn!("PUT {path} -> {status}: {body}");
            bail!("Firecracker API error: {status}");
        }

        Ok(())
    }
}
