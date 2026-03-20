use std::fs;
use std::process::Command;

use anyhow::{bail, Context, Result};
use log::{info, warn};

use crate::config::get;

pub struct NetworkManager;

impl NetworkManager {
    pub fn ensure_bridge(outbound_if: Option<&str>) -> Result<()> {
        let cfg = get();
        let bridge = &cfg.network.bridge;
        let bridge_ip = cfg.network.bridge_ip();
        let subnet = &cfg.network.subnet;

        if bridge_exists(bridge) {
            return Ok(());
        }

        info!("Creating bridge {bridge} ({bridge_ip}/24)...");

        run("ip", &["link", "add", bridge, "type", "bridge"])?;
        run(
            "ip",
            &["addr", "add", &format!("{bridge_ip}/24"), "dev", bridge],
        )?;
        run("ip", &["link", "set", bridge, "up"])?;

        fs::write("/proc/sys/net/ipv4/ip_forward", "1\n")?;

        let outbound = outbound_if
            .map(str::to_string)
            .unwrap_or_else(detect_default_interface);
        setup_nat(bridge, subnet, &outbound)
    }

    pub fn create_tap(tap_name: &str) -> Result<()> {
        let bridge = &get().network.bridge;
        info!("Creating tap device {tap_name}...");
        run("ip", &["tuntap", "add", "dev", tap_name, "mode", "tap"])?;
        run("ip", &["link", "set", tap_name, "master", bridge])?;
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

fn setup_nat(bridge: &str, subnet: &str, outbound_if: &str) -> Result<()> {
    let bridge_subnet = format!("{}.0/24", subnet);
    iptables_ensure(
        Some("nat"),
        "POSTROUTING",
        &["-s", &bridge_subnet, "-o", outbound_if, "-j", "MASQUERADE"],
    )?;
    iptables_ensure(
        None,
        "FORWARD",
        &["-i", bridge, "-o", outbound_if, "-j", "ACCEPT"],
    )?;
    iptables_ensure(
        None,
        "FORWARD",
        &[
            "-i",
            outbound_if,
            "-o",
            bridge,
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

fn bridge_exists(bridge: &str) -> bool {
    Command::new("ip")
        .args(["link", "show", bridge])
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
