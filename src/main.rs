use std::io::{self, Write};
use std::process::Command;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};

use vume::config;
use vume::network;
use vume::ssh::{run_in_vm, wait_for_ready};
use vume::state::{StateManager, VmStatus};
use vume::vm::VM;

#[derive(Parser)]
#[command(name = "vume", about = "Vume VM manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a VM. If --id matches a stopped VM then resume it.
    Start {
        /// VM identifier (default: auto-generated)
        #[arg(long)]
        id: Option<String>,
    },

    /// Stop a running VM
    Stop {
        /// VM ID
        id: String,
    },

    /// Execute a command in a running VM
    Exec {
        /// VM ID
        id: String,

        /// Command to execute
        command: String,

        /// Command timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,

        /// Skip waiting for SSH to be ready
        #[arg(long)]
        no_wait: bool,
    },

    /// Open an interactive SSH session to a VM
    Ssh {
        /// VM ID
        id: String,

        /// Skip waiting for SSH to be ready
        #[arg(long)]
        no_wait: bool,

        /// Enable SSH agent forwarding
        #[arg(short = 'A', long)]
        forward: bool,
    },

    /// Manage background processes in a VM
    Process {
        #[command(subcommand)]
        command: ProcessCommands,
    },

    /// Stop and remove a VM
    Destroy {
        /// VM ID (omit to destroy all)
        id: Option<String>,
    },

    /// List VMs
    List {
        /// Filter by status
        #[arg(long, value_parser = ["running", "stopped", "error"])]
        status: Option<String>,
    },

    /// Show or query configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Get a config value
    Get {
        /// Config key (e.g. kernel, firecracker, network.bridge)
        key: String,
    },

    /// Print the config file path
    Path,
}

#[derive(Subcommand)]
enum ProcessCommands {
    /// Start a background process in a VM
    Start {
        /// VM ID
        vm_id: String,
        /// Process name
        name: String,
        /// Command to run
        command: String,
        /// Working directory
        #[arg(long, default_value = "/root")]
        cwd: String,
        /// Disable auto-restart on failure
        #[arg(long)]
        no_restart: bool,
    },

    /// Stop and remove a background process
    Stop {
        /// VM ID
        vm_id: String,
        /// Process name
        name: String,
    },

    /// List background processes in a VM
    List {
        /// VM ID
        vm_id: String,
    },

    /// View logs for a background process
    Logs {
        /// VM ID
        vm_id: String,
        /// Process name
        name: String,
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: u32,
    },
}

fn main() {
    env_logger::init();

    let cfg = match config::Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("failed to load config: {e:#}");
            std::process::exit(1);
        }
    };
    config::init(cfg);

    if let Err(e) = run(Cli::parse()) {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Start { id } => {
            let mut vm = VM::new(id.as_deref())?;
            let info = vm.start()?;
            println!("id: {}", info.id);
            println!("pid: {}", info.pid);
            println!("ip: {}", info.ip);
            println!("status: {}", info.status);
        }

        Commands::Stop { id } => {
            VM::stop(&id)?;
        }

        Commands::Exec {
            id,
            command,
            timeout,
            no_wait,
        } => {
            let result = run_in_vm(&id, &[&command], !no_wait, timeout)?;
            if !result.stdout.is_empty() {
                print!("{}", result.stdout);
            }
            if !result.stderr.is_empty() {
                eprint!("{}", result.stderr);
            }
            std::process::exit(result.exit_code);
        }

        Commands::Ssh {
            id,
            no_wait,
            forward,
        } => {
            let state = StateManager::new()?;
            let vm = state
                .get_vm(&id)?
                .with_context(|| format!("VM {id} not found"))?;

            if vm.status != VmStatus::Running {
                bail!("VM {id} is not running (status: {})", vm.status);
            }

            if !no_wait && !wait_for_ready(&vm.ip, 30) {
                bail!("SSH not ready on {}", vm.ip);
            }

            let mut args = vec![
                "-i".to_string(),
                config::get().ssh_key.to_string_lossy().to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=no".to_string(),
                "-o".to_string(),
                "UserKnownHostsFile=/dev/null".to_string(),
                format!("root@{}", vm.ip),
            ];

            if forward {
                args.insert(0, "-A".to_string());
            }

            let status = Command::new("ssh")
                .args(&args)
                .status()
                .context("Failed to run ssh")?;

            std::process::exit(status.code().unwrap_or(1));
        }

        Commands::Process { command } => {
            match command {
                ProcessCommands::Start {
                    vm_id,
                    name,
                    command,
                    cwd,
                    no_restart,
                } => {
                    let name = sanitize_process_name(&name)?;
                    let restart_policy = if no_restart { "no" } else { "always" };
                    let unit = format!(
                        "[Unit]\n\
                     Description={name}\n\
                     \n\
                     [Service]\n\
                     Type=simple\n\
                     WorkingDirectory={cwd}\n\
                     ExecStart={command}\n\
                     Restart={restart_policy}\n\
                     RestartSec=2\n\
                     \n\
                     [Install]\n\
                     WantedBy=multi-user.target\n"
                    );

                    run_in_vm(
                        &vm_id,
                        &[
                            &format!("mkdir -p {cwd}"),
                            &format!("cat > /etc/systemd/system/vume-{name}.service << 'UNIT'\n{unit}UNIT"),
                            "systemctl daemon-reload",
                            &format!("systemctl enable --now vume-{name}.service"),
                        ],
                        true,
                        30,
                    )?;
                    println!("Started process '{name}' in VM {vm_id}");
                }

                ProcessCommands::Stop { vm_id, name } => {
                    let name = sanitize_process_name(&name)?;
                    run_in_vm(
                        &vm_id,
                        &[
                            &format!("sh -c 'systemctl disable --now vume-{name}.service || true'"),
                            &format!("rm -f /etc/systemd/system/vume-{name}.service"),
                            "systemctl daemon-reload",
                        ],
                        false,
                        30,
                    )?;
                    println!("Stopped process '{name}' in VM {vm_id}");
                }

                ProcessCommands::List { vm_id } => {
                    let result = run_in_vm(
                        &vm_id,
                        &["systemctl list-units 'vume-*' --type=service --no-pager --plain --all"],
                        false,
                        30,
                    )?;
                    if result.stdout.is_empty() {
                        println!("No managed processes found");
                    } else {
                        println!("{}", result.stdout);
                    }
                }

                ProcessCommands::Logs { vm_id, name, lines } => {
                    let cmd = format!("journalctl -u vume-{name} -n {lines} --no-pager");
                    let result = run_in_vm(&vm_id, &[&cmd], false, 30)?;
                    if !result.stdout.is_empty() {
                        print!("{}", result.stdout);
                    }
                }
            }
        }

        Commands::Destroy { id } => match id {
            Some(id) => VM::destroy(&id)?,
            None => {
                print!("This will destroy all VMs.  Are you sure? (y/n)\n> ");
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if input.trim().eq_ignore_ascii_case("y") {
                    let state = StateManager::new()?;
                    let vms = state.list_vms(None)?;
                    for vm in vms {
                        VM::destroy(&vm.id)?;
                    }
                }
            }
        },

        Commands::List { status } => {
            let state = StateManager::new()?;
            let stale = state.refresh_status()?;
            for item in &stale {
                network::delete_tap(&item.tap);
            }

            let filter = status.map(|s| s.parse::<VmStatus>()).transpose()?;
            let vms = state.list_vms(filter)?;
            if vms.is_empty() {
                println!("No VMs found.");
                return Ok(());
            }

            println!(
                "{:<12} {:<10} {:<16} {:<8} CREATED",
                "ID", "STATUS", "IP", "PID"
            );
            println!("{}", "-".repeat(70));
            for vm in &vms {
                let created = vm
                    .created_at
                    .get(..19)
                    .unwrap_or(&vm.created_at)
                    .replace('T', " ");
                println!(
                    "{:<12} {:<10} {:<16} {:<8} {created}",
                    vm.id, vm.status, vm.ip, vm.pid,
                );
            }
        }

        Commands::Config { command } => match command {
            ConfigCommands::Get { key } => {
                let cfg = config::get();
                match key.as_str() {
                    "home" => println!("{}", cfg.home.display()),
                    "kernel" => println!("{}", cfg.kernel.display()),
                    "firecracker" => println!("{}", cfg.firecracker.display()),
                    "ssh_key" => println!("{}", cfg.ssh_key.display()),
                    "zfs_pool" => println!("{}", cfg.zfs_pool),
                    "vcpu" => println!("{}", cfg.vcpu),
                    "mem" => println!("{}", cfg.mem),
                    "network.bridge" => println!("{}", cfg.network.bridge),
                    "network.subnet" => println!("{}", cfg.network.subnet),
                    "network.outbound_if" => {
                        println!("{}", cfg.network.outbound_if.as_deref().unwrap_or(""));
                    }
                    _ => bail!("Unknown config key: {key}"),
                }
            }
            ConfigCommands::Path => {
                println!("{}", config::vume_home().join("vume.toml").display());
            }
        },
    }

    Ok(())
}

fn sanitize_process_name(name: &str) -> anyhow::Result<&str> {
    if name.is_empty() {
        bail!("Process name cannot be empty");
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        bail!("Process name cannot start with a digit");
    }
    if name
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
    {
        bail!("Process name can only contain alphanumeric characters, dashes, and underscores");
    }
    Ok(name)
}
