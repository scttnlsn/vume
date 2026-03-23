use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Context;

static CONFIG: OnceLock<Config> = OnceLock::new();

const DEFAULT_VUME_HOME: &str = "/opt/vume";
const DEFAULT_VCPU: u32 = 2;
const DEFAULT_MEM: u32 = 1024;
const DEFAULT_ZFS_POOL: &str = "vume";
const DEFAULT_BRIDGE: &str = "br0";
const DEFAULT_SUBNET: &str = "172.16.0";
const IP_RANGE_START: u8 = 2;
const IP_RANGE_END: u8 = 254;

#[derive(Debug, Clone)]
pub struct Config {
    pub home: PathBuf,
    pub kernel: PathBuf,
    pub firecracker: PathBuf,
    pub ssh_key: PathBuf,
    pub zfs_pool: String,
    pub vcpu: u32,
    pub mem: u32,
    pub network: NetworkConfig,
}

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub bridge: String,
    pub subnet: String,
    pub outbound_if: Option<String>,
}

impl Config {
    pub fn vms_dir(&self) -> PathBuf {
        self.home.join("vms")
    }

    pub fn db_path(&self) -> PathBuf {
        self.home.join("vume.db")
    }

    pub fn zvol_path(&self, vm_id: &str) -> String {
        format!("/dev/zvol/{}/{}", self.zfs_pool, vm_id)
    }

    pub fn zfs_dataset(&self, name: &str) -> String {
        format!("{}/{}", self.zfs_pool, name)
    }
}

impl NetworkConfig {
    pub fn bridge_ip(&self) -> String {
        format!("{}.1", self.subnet)
    }

    pub fn bridge_cidr(&self) -> String {
        format!("{}.0/24", self.subnet)
    }

    pub fn allocate_ip(&self, used: &HashSet<String>) -> Option<String> {
        for i in IP_RANGE_START..=IP_RANGE_END {
            let candidate = format!("{}.{}", self.subnet, i);
            if !used.contains(&candidate) {
                return Some(candidate);
            }
        }
        None
    }
}

pub fn vume_home() -> PathBuf {
    std::env::var("VUME_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_VUME_HOME))
}

fn config_path() -> PathBuf {
    vume_home().join("vume.toml")
}

#[derive(serde::Deserialize)]
struct RawConfig {
    kernel: Option<PathBuf>,
    firecracker: Option<PathBuf>,
    ssh_key: Option<PathBuf>,
    zfs_pool: Option<String>,
    vcpu: Option<u32>,
    mem: Option<u32>,
    network: Option<RawNetworkConfig>,
}

#[derive(Default, serde::Deserialize)]
struct RawNetworkConfig {
    bridge: Option<String>,
    subnet: Option<String>,
    outbound_if: Option<String>,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let home = vume_home();
        let path = config_path();

        let raw: RawConfig = if path.exists() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read config file: {}", path.display()))?;
            toml::from_str(&content)
                .with_context(|| format!("Failed to parse config file: {}", path.display()))?
        } else {
            RawConfig {
                kernel: None,
                firecracker: None,
                ssh_key: None,
                zfs_pool: None,
                vcpu: None,
                mem: None,
                network: None,
            }
        };

        let kernel = raw.kernel.unwrap_or_else(|| home.join("vmlinux"));
        let firecracker = raw.firecracker.unwrap_or_else(|| home.join("firecracker"));
        let ssh_key = raw.ssh_key.unwrap_or_else(|| home.join("vume_key"));
        let zfs_pool = raw.zfs_pool.unwrap_or_else(|| DEFAULT_ZFS_POOL.to_string());
        let vcpu = raw.vcpu.unwrap_or(DEFAULT_VCPU);
        let mem = raw.mem.unwrap_or(DEFAULT_MEM);

        let raw_net = raw.network.unwrap_or_default();
        let network = NetworkConfig {
            bridge: raw_net.bridge.unwrap_or_else(|| DEFAULT_BRIDGE.to_string()),
            subnet: raw_net.subnet.unwrap_or_else(|| DEFAULT_SUBNET.to_string()),
            outbound_if: raw_net.outbound_if,
        };

        Ok(Config {
            home,
            kernel,
            firecracker,
            ssh_key,
            zfs_pool,
            vcpu,
            mem,
            network,
        })
    }
}

pub fn get() -> &'static Config {
    CONFIG.get().expect("config not initialized")
}

pub fn init(cfg: Config) {
    CONFIG.set(cfg).expect("config already initialized");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_no_env_no_file() {
        std::env::remove_var("VUME_HOME");
        let cfg = Config::load().unwrap();

        assert_eq!(cfg.home, PathBuf::from("/opt/vume"));
        assert_eq!(cfg.kernel, PathBuf::from("/opt/vume/vmlinux"));
        assert_eq!(cfg.firecracker, PathBuf::from("/opt/vume/firecracker"));
        assert_eq!(cfg.ssh_key, PathBuf::from("/opt/vume/vume_key"));
        assert_eq!(cfg.zfs_pool, "vume");
        assert_eq!(cfg.vcpu, 2);
        assert_eq!(cfg.mem, 1024);
        assert_eq!(cfg.network.bridge, "br0");
        assert_eq!(cfg.network.subnet, "172.16.0");
        assert!(cfg.network.outbound_if.is_none());
    }

    #[test]
    fn test_vume_home_env() {
        std::env::set_var("VUME_HOME", "/custom/path");
        let cfg = Config::load().unwrap();
        std::env::remove_var("VUME_HOME");

        assert_eq!(cfg.home, PathBuf::from("/custom/path"));
        assert_eq!(cfg.kernel, PathBuf::from("/custom/path/vmlinux"));
        assert_eq!(cfg.firecracker, PathBuf::from("/custom/path/firecracker"));
        assert_eq!(cfg.ssh_key, PathBuf::from("/custom/path/vume_key"));
    }

    #[test]
    fn test_network_accessors() {
        let nc = NetworkConfig {
            bridge: "br0".to_string(),
            subnet: "10.0.0".to_string(),
            outbound_if: Some("eth1".to_string()),
        };

        assert_eq!(nc.bridge_ip(), "10.0.0.1");
        assert_eq!(nc.bridge_cidr(), "10.0.0.0/24");
    }

    #[test]
    fn test_allocate_ip() {
        let nc = NetworkConfig {
            bridge: "br0".to_string(),
            subnet: "172.16.0".to_string(),
            outbound_if: None,
        };

        assert_eq!(nc.allocate_ip(&HashSet::new()).unwrap(), "172.16.0.2");
        assert_eq!(
            nc.allocate_ip(&HashSet::from(["172.16.0.2".to_string()]))
                .unwrap(),
            "172.16.0.3"
        );
        assert_eq!(
            nc.allocate_ip(&HashSet::from([
                "172.16.0.2".to_string(),
                "172.16.0.3".to_string()
            ]))
            .unwrap(),
            "172.16.0.4"
        );
    }

    #[test]
    fn test_config_accessors() {
        let cfg = Config::load().unwrap();
        assert_eq!(cfg.vms_dir(), PathBuf::from("/opt/vume/vms"));
        assert_eq!(cfg.db_path(), PathBuf::from("/opt/vume/vume.db"));
        assert_eq!(cfg.zvol_path("vm123"), "/dev/zvol/vume/vm123");
        assert_eq!(cfg.zfs_dataset("foo"), "vume/foo");
    }
}
