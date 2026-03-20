use std::io::Read;
use std::net::TcpStream;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ssh2::Session;

use crate::config::get;
use crate::state::{StateManager, VmStatus};

#[derive(Debug)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

struct SSHClient {
    session: Session,
}

impl SSHClient {
    fn connect(ip: &str, timeout: Duration) -> Result<Self> {
        let addr = format!("{ip}:22").parse().context("Invalid SSH address")?;

        let tcp =
            TcpStream::connect_timeout(&addr, timeout).context("Failed to connect via TCP")?;

        let mut session = Session::new().context("Failed to create SSH session")?;
        session.set_timeout(timeout.as_millis() as u32);
        session.set_tcp_stream(tcp);
        session.handshake().context("SSH handshake failed")?;

        session
            .userauth_pubkey_file("root", None, &get().ssh_key, None)
            .context("SSH key authentication failed")?;

        Ok(Self { session })
    }

    fn exec(&self, command: &str, timeout: Duration) -> Result<CommandResult> {
        self.session
            .set_timeout(timeout.as_millis().min(u32::MAX as u128) as u32);

        let mut channel = self
            .session
            .channel_session()
            .context("Failed to open channel")?;
        channel.exec(command).context("Failed to exec command")?;

        let mut stdout = String::new();
        channel
            .read_to_string(&mut stdout)
            .context("Failed to read stdout")?;

        let mut stderr = String::new();
        channel
            .stderr()
            .read_to_string(&mut stderr)
            .context("Failed to read stderr")?;

        channel.wait_close().ok();
        let exit_code = channel.exit_status().unwrap_or(-1);

        Ok(CommandResult {
            exit_code,
            stdout,
            stderr,
        })
    }
}

pub fn wait_for_ready(ip: &str, timeout_secs: u64) -> bool {
    let start = Instant::now();
    let deadline = Duration::from_secs(timeout_secs);

    while start.elapsed() < deadline {
        // Flush ARP cache
        let _ = Command::new("ip")
            .args(["neigh", "flush", "to", ip])
            .output();

        if SSHClient::connect(ip, Duration::from_secs(2)).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

pub fn run_in_vm(
    vm_id: &str,
    commands: &[&str],
    wait: bool,
    timeout_secs: u64,
) -> Result<CommandResult> {
    if commands.is_empty() {
        bail!("No commands to execute");
    }

    let state = StateManager::new()?;
    let vm = state
        .get_vm(vm_id)?
        .with_context(|| format!("VM {vm_id} not found"))?;

    if vm.status != VmStatus::Running {
        bail!("VM {vm_id} is not running (status: {})", vm.status);
    }
    let ip = vm.ip;

    let timeout = Duration::from_secs(timeout_secs);

    if wait && !wait_for_ready(&ip, timeout_secs) {
        bail!("SSH not ready on {ip}");
    }

    let client = SSHClient::connect(&ip, timeout)?;

    let mut last = None;
    for cmd in commands {
        let result = client.exec(cmd, timeout)?;
        if result.exit_code != 0 {
            return Ok(result);
        }
        last = Some(result);
    }

    Ok(last.expect("commands was non-empty"))
}
