use std::fs;
use std::process::Command;

use anyhow::{bail, Context, Result};
use log::{info, warn};

pub const BRIDGE: &str = "br0";
pub const BRIDGE_IP: &str = "172.16.0.1";
const BRIDGE_SUBNET: &str = "172.16.0.0/24";
const BRIDGE_PREFIXLEN: u8 = 24;

pub struct NetworkManager;

impl NetworkManager {
    pub fn ensure_bridge(outbound_if: Option<&str>) -> Result<()> {
        if bridge_exists() {
            return Ok(());
        }

        info!("Creating bridge {BRIDGE} ({BRIDGE_IP}/{BRIDGE_PREFIXLEN})...");

        run("ip", &["link", "add", BRIDGE, "type", "bridge"])?;
        run(
            "ip",
            &[
                "addr",
                "add",
                &format!("{BRIDGE_IP}/{BRIDGE_PREFIXLEN}"),
                "dev",
                BRIDGE,
            ],
        )?;
        run("ip", &["link", "set", BRIDGE, "up"])?;

        fs::write("/proc/sys/net/ipv4/ip_forward", "1\n")?;

        let outbound = outbound_if
            .map(str::to_string)
            .unwrap_or_else(detect_default_interface);
        setup_nat(&outbound)
    }

    pub fn create_tap(tap_name: &str) -> Result<()> {
        info!("Creating tap device {tap_name}...");
        run("ip", &["tuntap", "add", "dev", tap_name, "mode", "tap"])?;
        run("ip", &["link", "set", tap_name, "master", BRIDGE])?;
        run("ip", &["link", "set", tap_name, "up"])
    }

    pub fn delete_tap(tap_name: &str) {
        if let Err(e) = Command::new("ip").args(["link", "del", tap_name]).output() {
            warn!("Failed to remove tap device {tap_name}: {e}");
        } else {
            info!("Removed tap device {tap_name}");
        }
    }
}

fn setup_nat(outbound_if: &str) -> Result<()> {
    iptables_ensure(
        Some("nat"),
        "POSTROUTING",
        &["-s", BRIDGE_SUBNET, "-o", outbound_if, "-j", "MASQUERADE"],
    )?;
    iptables_ensure(
        None,
        "FORWARD",
        &["-i", BRIDGE, "-o", outbound_if, "-j", "ACCEPT"],
    )?;
    iptables_ensure(
        None,
        "FORWARD",
        &[
            "-i",
            outbound_if,
            "-o",
            BRIDGE,
            "-m",
            "state",
            "--state",
            "RELATED,ESTABLISHED",
            "-j",
            "ACCEPT",
        ],
    )?;
    Ok(())
}

/// Add an iptables rule if it doesn't already exist.
fn iptables_ensure(table: Option<&str>, chain: &str, args: &[&str]) -> Result<()> {
    let mut check_cmd = Vec::new();
    let mut add_cmd = Vec::new();

    if let Some(t) = table {
        check_cmd.extend_from_slice(&["-t", t]);
        add_cmd.extend_from_slice(&["-t", t]);
    }

    check_cmd.extend_from_slice(&["-C", chain]);
    add_cmd.extend_from_slice(&["-A", chain]);

    check_cmd.extend_from_slice(args);
    add_cmd.extend_from_slice(args);

    let exists = Command::new("iptables")
        .args(&check_cmd)
        .output()
        .context("Failed to run iptables")?
        .status
        .success();

    if !exists {
        run("iptables", &add_cmd)?;
    }
    Ok(())
}

fn bridge_exists() -> bool {
    Command::new("ip")
        .args(["link", "show", BRIDGE])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn detect_default_interface() -> String {
    let Ok(output) = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
    else {
        warn!("Failed to run 'ip route show default', falling back to eth0");
        return "eth0".into();
    };

    if !output.status.success() {
        warn!(
            "'ip route show default' exited with {}, falling back to eth0",
            output.status
        );
        return "eth0".into();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let Some(idx) = parts.iter().position(|&p| p == "dev") {
            if let Some(&iface) = parts.get(idx + 1) {
                if iface != "lo" {
                    return iface.to_string();
                }
            }
        }
    }

    warn!("No default route found, falling back to eth0");
    "eth0".into()
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Failed to run {program}"))?;
    if !status.success() {
        bail!("{program} {} failed with {status}", args.join(" "));
    }
    Ok(())
}
