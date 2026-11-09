use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMethod {
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "key")]
    KeyFile { path: PathBuf },
    #[serde(rename = "password")]
    Password { env_var: Option<String> },
}

impl Default for AuthMethod {
    fn default() -> Self {
        Self::Agent
    }
}

/// Where a server entry came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NodeSource {
    #[default]
    Fleet,
    SshConfig,
}

impl NodeSource {
    pub fn label(self) -> &'static str {
        match self {
            NodeSource::Fleet => "corex",
            NodeSource::SshConfig => "ssh",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_user")]
    pub user: String,
    #[serde(default)]
    pub key_path: Option<PathBuf>,
    #[serde(default)]
    pub auth: AuthMethod,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub sudo: bool,
    #[serde(default)]
    pub proxy_jump: Option<String>,
    #[serde(skip)]
    pub source: NodeSource,
}

fn default_port() -> u16 {
    22
}

fn default_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".to_string())
}

pub fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" {
        if let Some(base) = BaseDirs::new() {
            return base.home_dir().to_path_buf();
        }
    } else if let Some(stripped) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(base) = BaseDirs::new() {
            return base.home_dir().join(stripped);
        }
    }
    path.to_path_buf()
}

fn bracketed_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]", host)
    } else {
        host.to_string()
    }
}

impl NodeConfig {
    pub fn new(id: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            host: host.into(),
            port: default_port(),
            user: default_user(),
            key_path: None,
            auth: AuthMethod::Agent,
            tags: Vec::new(),
            sudo: false,
            proxy_jump: None,
            source: NodeSource::Fleet,
        }
    }

    pub fn is_user_owned(&self) -> bool {
        self.source == NodeSource::SshConfig
    }

    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    pub fn target_string(&self) -> String {
        format!("{}@{}:{}", self.user, bracketed_host(&self.host), self.port)
    }

    pub fn resolved_key_path(&self) -> Option<PathBuf> {
        let raw = if let Some(ref kp) = self.key_path {
            Some(kp.clone())
        } else if let AuthMethod::KeyFile { ref path } = self.auth {
            Some(path.clone())
        } else {
            None
        };
        raw.map(|p| expand_tilde(&p))
    }

    pub fn normalize_paths(&mut self) {
        if let Some(ref kp) = self.key_path {
            self.key_path = Some(expand_tilde(kp));
        }
        if let AuthMethod::KeyFile { ref mut path } = self.auth {
            *path = expand_tilde(path);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Defaults to the table key (`[clusters.<id>]`).
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
}

impl ClusterConfig {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            nodes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetConfig {
    #[serde(default)]
    pub default_cluster: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub clusters: HashMap<String, ClusterConfig>,
    #[serde(default = "default_standalone_nodes")]
    pub nodes: Vec<NodeConfig>,
    #[serde(default = "default_safe_commands")]
    pub safe_commands: Vec<String>,
    #[serde(default = "default_dangerous_commands")]
    pub dangerous_commands: Vec<String>,
    /// Loaded from the workspace `.corex/fleet.toml`, which arrives with the repository: its
    /// servers are only reached once the user approves it.
    #[serde(skip)]
    pub from_workspace: bool,
}

fn default_timeout_secs() -> u64 {
    30
}

fn default_standalone_nodes() -> Vec<NodeConfig> {
    Vec::new()
}

fn default_safe_commands() -> Vec<String> {
    vec![
        "uptime".to_string(),
        "uname".to_string(),
        "df".to_string(),
        "free".to_string(),
        "cat".to_string(),
        "ps".to_string(),
        "top".to_string(),
        "htop".to_string(),
        "docker ps".to_string(),
        "systemctl".to_string(),
        "systemctl status".to_string(),
        "journalctl".to_string(),
        "netstat".to_string(),
        "ss".to_string(),
        // Read-only `ip` subcommands only; `ip link set`, `ip route del`, … are mutating and are
        // additionally caught by the flag heuristics in ssh.rs.
        "ip addr".to_string(),
        "ip address".to_string(),
        "ip route".to_string(),
        "ip link show".to_string(),
        "ip -br".to_string(),
        "whoami".to_string(),
        "pwd".to_string(),
        "ls".to_string(),
        "echo".to_string(),
        "lscpu".to_string(),
        "lsblk".to_string(),
        "lshw".to_string(),
        "lspci".to_string(),
        "lsmod".to_string(),
        "grep".to_string(),
        "head".to_string(),
        "tail".to_string(),
        "wc".to_string(),
        "date".to_string(),
        "arch".to_string(),
        "hostname".to_string(),
        "hostnamectl".to_string(),
        "timedatectl".to_string(),
        "localectl".to_string(),
        "systemd-detect-virt".to_string(),
        "lsb_release".to_string(),
        "nproc".to_string(),
        "du".to_string(),
        "id".to_string(),
        "which".to_string(),
        "whereis".to_string(),
        "pstree".to_string(),
        "pgrep".to_string(),
        "column".to_string(),
        "cut".to_string(),
        "tr".to_string(),
        "sort".to_string(),
        "uniq".to_string(),
        "awk".to_string(),
        "lsof".to_string(),
        "vmstat".to_string(),
        "iostat".to_string(),
        "mpstat".to_string(),
        "dmesg".to_string(),
        "findmnt".to_string(),
    ]
}

fn default_dangerous_commands() -> Vec<String> {
    vec![
        "rm -rf".to_string(),
        "reboot".to_string(),
        "shutdown".to_string(),
        "init 0".to_string(),
        "poweroff".to_string(),
        "mkfs".to_string(),
        "dd".to_string(),
        "iptables -F".to_string(),
        "killall".to_string(),
        "> /dev/sda".to_string(),
    ]
}

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            default_cluster: None,
            timeout_secs: default_timeout_secs(),
            clusters: HashMap::new(),
            nodes: Vec::new(),
            safe_commands: default_safe_commands(),
            dangerous_commands: default_dangerous_commands(),
            from_workspace: false,
        }
    }
}

impl FleetConfig {
    fn normalize(&mut self) {
        for node in &mut self.nodes {
            node.normalize_paths();
        }
        for (key, cluster) in self.clusters.iter_mut() {
            if cluster.id.is_empty() {
                cluster.id = key.clone();
            }
            for node in &mut cluster.nodes {
                node.normalize_paths();
            }
        }
    }

    /// Where `fleet.toml` is looked for, first match wins: the workspace `.corex/`, then
    /// `~/.corex/`, then `~/.config/corex/`.
    fn candidate_paths(workspace: Option<&Path>) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(ws) = workspace {
            paths.push(ws.join(".corex").join("fleet.toml"));
        }
        if let Some(base) = BaseDirs::new() {
            paths.push(base.home_dir().join(".corex").join("fleet.toml"));
            paths.push(base.config_dir().join("corex").join("fleet.toml"));
        }
        paths
    }

    /// The command lists of the user's own `fleet.toml`, or the defaults.
    fn global_command_lists() -> (Vec<String>, Vec<String>) {
        Self::candidate_paths(None)
            .into_iter()
            .find(|p| p.exists())
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|c| Self::parse(&c).ok())
            .map(|cfg| (cfg.safe_commands, cfg.dangerous_commands))
            .unwrap_or_else(|| (default_safe_commands(), default_dangerous_commands()))
    }

    /// Reads one candidate file. A workspace file may list servers but never widens what runs
    /// unasked: its command lists give way to the user's own and its servers need approval.
    fn read_candidate(path: &Path, workspace: Option<&Path>) -> Result<Self> {
        let content = fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
        let mut cfg = Self::parse(&content).with_context(|| format!("Failed to parse TOML in {}", path.display()))?;
        if workspace.is_some_and(|ws| path == ws.join(".corex").join("fleet.toml")) {
            (cfg.safe_commands, cfg.dangerous_commands) = Self::global_command_lists();
            cfg.from_workspace = true;
        }
        Ok(cfg)
    }

    pub fn parse(content: &str) -> Result<Self> {
        let mut cfg: FleetConfig = toml::from_str(content)?;
        cfg.normalize();
        Ok(cfg)
    }

    pub fn load(workspace: Option<&Path>) -> Result<Self> {
        let mut cfg = match Self::candidate_paths(workspace).into_iter().find(|p| p.exists()) {
            Some(path) => Self::read_candidate(&path, workspace)?,
            None => FleetConfig::default(),
        };
        cfg.merge_discovered(Self::discover_ssh_config().unwrap_or_default());
        cfg.normalize();

        Ok(cfg)
    }

    /// Loader for safety checks: never fails, still discovers `~/.ssh/config`.
    pub fn load_fast(workspace: Option<&Path>) -> Self {
        let mut cfg = Self::candidate_paths(workspace)
            .into_iter()
            .find(|p| p.exists())
            .and_then(|path| Self::read_candidate(&path, workspace).ok())
            .unwrap_or_default();
        cfg.merge_discovered(Self::discover_ssh_config().unwrap_or_default());
        cfg.normalize();

        cfg
    }

    pub fn merge_discovered(&mut self, discovered: Vec<NodeConfig>) {
        for node in discovered {
            let claimed = self.nodes.iter().any(|n| n.id == node.id)
                || self
                    .clusters
                    .values()
                    .any(|c| c.nodes.iter().any(|n| n.id == node.id));
            if !claimed {
                self.nodes.push(node);
            }
        }
    }

    pub fn node_is_untrusted(&self, node: &NodeConfig) -> bool {
        self.from_workspace && !node.is_user_owned()
    }

    pub fn has_untrusted_nodes(&self) -> bool {
        self.from_workspace
            && (self.nodes.iter().any(|n| !n.is_user_owned())
                || self
                    .clusters
                    .values()
                    .any(|c| c.nodes.iter().any(|n| !n.is_user_owned())))
    }

    /// Saves configuration to workspace `.corex/fleet.toml` or global `~/.config/corex/fleet.toml`.
    pub fn save(&self, workspace: Option<&Path>) -> Result<PathBuf> {
        let target_path = if let Some(ws) = workspace {
            let dir = ws.join(".corex");
            corex_core::ensure_private_dir(&dir)
                .with_context(|| format!("Failed to create directory {}", dir.display()))?;
            dir.join("fleet.toml")
        } else if let Some(base) = BaseDirs::new() {
            let dir = base.config_dir().join("corex");
            corex_core::ensure_private_dir(&dir)
                .with_context(|| format!("Failed to create directory {}", dir.display()))?;
            dir.join("fleet.toml")
        } else {
            PathBuf::from("fleet.toml")
        };

        let mut persistable = self.clone();
        persistable.nodes.retain(|n| n.source == NodeSource::Fleet);
        for cluster in persistable.clusters.values_mut() {
            cluster.nodes.retain(|n| n.source == NodeSource::Fleet);
        }

        let serialized = toml::to_string_pretty(&persistable)
            .context("Failed to serialize fleet configuration to TOML")?;
        corex_core::write_private_atomic(&target_path, serialized)
            .with_context(|| format!("Failed to write to {}", target_path.display()))?;
        Ok(target_path)
    }

    /// Finds a node by ID either in standalone nodes or inside any cluster.
    pub fn find_node(&self, node_id: &str) -> Option<&NodeConfig> {
        if let Some(node) = self.nodes.iter().find(|n| n.id == node_id) {
            return Some(node);
        }
        let mut cluster_keys: Vec<&String> = self.clusters.keys().collect();
        cluster_keys.sort();
        for key in cluster_keys {
            if let Some(cluster) = self.clusters.get(key) {
                if let Some(node) = cluster.nodes.iter().find(|n| n.id == node_id) {
                    return Some(node);
                }
            }
        }
        None
    }

    /// Returns all configured nodes across standalone list and all clusters in deterministic order.
    pub fn all_nodes(&self) -> Vec<&NodeConfig> {
        let mut list = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for node in &self.nodes {
            if seen.insert(&node.id) {
                list.push(node);
            }
        }
        let mut cluster_keys: Vec<&String> = self.clusters.keys().collect();
        cluster_keys.sort();
        for key in cluster_keys {
            if let Some(cluster) = self.clusters.get(key) {
                for node in &cluster.nodes {
                    if seen.insert(&node.id) {
                        list.push(node);
                    }
                }
            }
        }
        list
    }

    /// Returns a default or single node if unambiguous:
    /// checks if `all_nodes().len() == 1`, or if `default_cluster` is set and that cluster has 1 node.
    pub fn default_or_single_node(&self) -> Option<&NodeConfig> {
        let all = self.all_nodes();
        if all.len() == 1 {
            return Some(all[0]);
        }
        if let Some(ref cluster_id) = self.default_cluster {
            if let Some(cluster) = self.clusters.get(cluster_id) {
                if cluster.nodes.len() == 1 {
                    return Some(&cluster.nodes[0]);
                }
            }
        }
        None
    }

    /// Filter nodes by tag (e.g. "web", "db", "prod").
    pub fn nodes_by_tag(&self, tag: &str) -> Vec<&NodeConfig> {
        self.all_nodes()
            .into_iter()
            .filter(|n| n.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
            .collect()
    }

    /// Automatically scans and parses `~/.ssh/config` to discover configured hosts.
    pub fn discover_ssh_config() -> Result<Vec<NodeConfig>> {
        let Some(base) = BaseDirs::new() else {
            return Ok(Vec::new());
        };
        let ssh_config_path = base.home_dir().join(".ssh").join("config");
        if !ssh_config_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&ssh_config_path)?;
        Ok(Self::parse_ssh_config_str(&content))
    }

    /// Parses OpenSSH config content into `NodeConfig` entries.
    pub fn parse_ssh_config_str(content: &str) -> Vec<NodeConfig> {
        let mut discovered = Vec::new();

        let mut current_hosts: Vec<String> = Vec::new();
        let mut current_hostname: Option<String> = None;
        let mut current_user: Option<String> = None;
        let mut current_port: Option<u16> = None;
        let mut current_key: Option<PathBuf> = None;
        let mut current_jump: Option<String> = None;

        let commit_current = |discovered: &mut Vec<NodeConfig>,
                              hosts: &mut Vec<String>,
                              hostname: &mut Option<String>,
                              user: &mut Option<String>,
                              port: &mut Option<u16>,
                              key: &mut Option<PathBuf>,
                              jump: &mut Option<String>| {
            let taken_hosts = std::mem::take(hosts);
            let hn = hostname.take();
            let u = user.take();
            let p = port.take();
            let k = key.take();
            let j = jump.take();

            for h in taken_hosts {
                if !h.is_empty() && !h.contains('*') && !h.contains('?') && !h.starts_with('!') {
                    let real_host = hn.clone().unwrap_or_else(|| h.clone());
                    let resolved_user = u.clone().unwrap_or_else(default_user);
                    let resolved_port = p.unwrap_or(22);

                    let mut node = NodeConfig::new(h, real_host);
                    node.user = resolved_user;
                    node.port = resolved_port;
                    node.key_path = k.clone();
                    node.proxy_jump = j.clone();
                    node.source = NodeSource::SshConfig;
                    node.tags.push("ssh_config".to_string());
                    discovered.push(node);
                }
            }
        };

        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = match line.find('#') {
                Some(idx) => line[..idx].trim(),
                None => line,
            };
            if line.is_empty() {
                continue;
            }

            let (keyword, rest) = match line.find(|c: char| c.is_whitespace() || c == '=') {
                Some(idx) => {
                    let kw = &line[..idx];
                    let after = line[idx..].trim_start();
                    let after = after.strip_prefix('=').unwrap_or(after).trim();
                    (kw, after)
                }
                None => continue,
            };
            if rest.is_empty() {
                continue;
            }

            if keyword.eq_ignore_ascii_case("Match") {
                commit_current(
                    &mut discovered,
                    &mut current_hosts,
                    &mut current_hostname,
                    &mut current_user,
                    &mut current_port,
                    &mut current_key,
                    &mut current_jump,
                );
                // Ignore everything inside Match block until the next Host directive
                current_hosts.clear();
            } else if keyword.eq_ignore_ascii_case("Host") {
                commit_current(
                    &mut discovered,
                    &mut current_hosts,
                    &mut current_hostname,
                    &mut current_user,
                    &mut current_port,
                    &mut current_key,
                    &mut current_jump,
                );
                current_hosts = rest
                    .split_whitespace()
                    .map(|s| s.trim_matches('"').trim_matches('\'').to_string())
                    .filter(|h| !h.is_empty() && !h.contains('*') && !h.contains('?') && !h.starts_with('!'))
                    .collect();
            } else if !current_hosts.is_empty() {
                let value = if (rest.starts_with('"') && rest.ends_with('"') && rest.len() >= 2)
                    || (rest.starts_with('\'') && rest.ends_with('\'') && rest.len() >= 2)
                {
                    &rest[1..rest.len() - 1]
                } else {
                    rest.split_whitespace().next().unwrap_or("")
                };
                if value.is_empty() {
                    continue;
                }

                if keyword.eq_ignore_ascii_case("HostName") {
                    if current_hostname.is_none() {
                        current_hostname = Some(value.to_string());
                    }
                } else if keyword.eq_ignore_ascii_case("User") {
                    if current_user.is_none() {
                        current_user = Some(value.to_string());
                    }
                } else if keyword.eq_ignore_ascii_case("Port") {
                    if current_port.is_none() {
                        if let Ok(p) = value.parse::<u16>() {
                            current_port = Some(p);
                        }
                    }
                } else if keyword.eq_ignore_ascii_case("IdentityFile") {
                    if current_key.is_none() {
                        current_key = Some(expand_tilde(Path::new(value)));
                    }
                } else if keyword.eq_ignore_ascii_case("ProxyJump") {
                    if current_jump.is_none() {
                        current_jump = Some(value.to_string());
                    }
                }
            }
        }

        // Commit final host block
        commit_current(
            &mut discovered,
            &mut current_hosts,
            &mut current_hostname,
            &mut current_user,
            &mut current_port,
            &mut current_key,
            &mut current_jump,
        );

        discovered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fleet_toml_serialization() {
        let mut config = FleetConfig::default();
        let mut node = NodeConfig::new("web-01", "192.168.1.10");
        node.tags = vec!["web".to_string(), "production".to_string()];
        config.nodes.push(node);

        let serialized = toml::to_string(&config).unwrap();
        let deserialized: FleetConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.nodes.len(), 1);
        assert_eq!(deserialized.nodes[0].id, "web-01");
        assert_eq!(deserialized.nodes[0].host, "192.168.1.10");
        assert_eq!(deserialized.nodes[0].tags, vec!["web", "production"]);
        assert!(deserialized.find_node("web-01").is_some());
    }

    #[test]
    fn test_expand_tilde_and_resolved_key_path() {
        let expanded = expand_tilde(Path::new("~/.ssh/id_ed25519"));
        if let Some(base) = BaseDirs::new() {
            assert_eq!(expanded, base.home_dir().join(".ssh/id_ed25519"));
        }

        let mut node = NodeConfig::new("srv-1", "10.0.0.1");
        assert!(node.resolved_key_path().is_none());

        node.auth = AuthMethod::KeyFile {
            path: PathBuf::from("~/.ssh/fallback_key"),
        };
        let fallback = node.resolved_key_path().unwrap();
        assert!(!fallback.to_string_lossy().starts_with("~/"));
        assert!(fallback.to_string_lossy().ends_with(".ssh/fallback_key"));

        node.key_path = Some(PathBuf::from("~/.ssh/primary_key"));
        let primary = node.resolved_key_path().unwrap();
        assert!(!primary.to_string_lossy().starts_with("~/"));
        assert!(primary.to_string_lossy().ends_with(".ssh/primary_key"));
    }

    #[test]
    fn test_default_or_single_node() {
        let mut config = FleetConfig::default();
        assert!(config.default_or_single_node().is_none());

        config.nodes.push(NodeConfig::new("solo", "10.0.0.1"));
        assert_eq!(config.default_or_single_node().unwrap().id, "solo");

        config.nodes.push(NodeConfig::new("second", "10.0.0.2"));
        assert!(config.default_or_single_node().is_none());

        let mut cluster = ClusterConfig::new("staging", "Staging");
        cluster.nodes.push(NodeConfig::new("stg-1", "10.0.1.1"));
        config.clusters.insert("staging".to_string(), cluster);
        config.default_cluster = Some("staging".to_string());
        assert_eq!(config.default_or_single_node().unwrap().id, "stg-1");
    }

    #[test]
    fn test_discover_ssh_config_parsing() {
        let ssh_cfg = r#"
# Global defaults
Host *
    User globaluser
    Port 2222

Host web-01 web-02 *.internal !excluded
    HostName=10.10.0.5
    HostName 10.10.0.99
    User = deploy
    User = ignored_second_user
    Port=2200
    Port 9999
    IdentityFile ~/.ssh/id_ed25519
    IdentityFile ~/.ssh/id_rsa_backup
    ProxyJump=bastion.example.com

Match host 192.168.*
    User matchuser
    HostName 192.168.1.99
    Port 3333

Host db-prod
    HostName 10.20.0.10
    User postgres
"#;

        let nodes = FleetConfig::parse_ssh_config_str(ssh_cfg);
        assert_eq!(nodes.len(), 3);

        assert_eq!(nodes[0].id, "web-01");
        assert_eq!(nodes[0].host, "10.10.0.5");
        assert_eq!(nodes[0].user, "deploy");
        assert_eq!(nodes[0].port, 2200);
        assert_eq!(nodes[0].proxy_jump.as_deref(), Some("bastion.example.com"));
        assert!(nodes[0]
            .key_path
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .ends_with(".ssh/id_ed25519"));
        assert!(!nodes[0]
            .key_path
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .starts_with("~/"));

        assert_eq!(nodes[1].id, "web-02");
        assert_eq!(nodes[1].host, "10.10.0.5");
        assert_eq!(nodes[1].user, "deploy");
        assert_eq!(nodes[1].port, 2200);

        assert_eq!(nodes[2].id, "db-prod");
        assert_eq!(nodes[2].host, "10.20.0.10");
        assert_eq!(nodes[2].user, "postgres");
        assert_eq!(nodes[2].port, 22);
    }

    #[test]
    fn cluster_id_defaults_to_its_table_key() {
        let cfg = FleetConfig::parse(
            "[clusters.production]\nname = \"Prod\"\n\n[[clusters.production.nodes]]\nid = \"web\"\nhost = \"10.0.0.1\"\nuser = \"root\"\n",
        )
        .expect("a cluster without `id` parses");
        assert_eq!(cfg.clusters["production"].id, "production");
    }

    #[test]
    fn test_workspace_fleet_toml_cannot_override_safe_or_dangerous_commands() {
        let ws = std::env::temp_dir().join(format!("corex_fleet_cfg_f01_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        let corex_dir = ws.join(".corex");
        std::fs::create_dir_all(&corex_dir).unwrap();
        std::fs::write(
            corex_dir.join("fleet.toml"),
            r#"
safe_commands = ["sh", "bash", "rm -rf"]
dangerous_commands = []

[[nodes]]
id = "web-01"
host = "10.0.0.1"
"#,
        )
        .unwrap();

        let loaded = FleetConfig::load(Some(&ws)).unwrap();
        let fast = FleetConfig::load_fast(Some(&ws));
        let _ = std::fs::remove_dir_all(&ws);

        assert_eq!(
            loaded.nodes.iter().filter(|n| n.source == NodeSource::Fleet).count(),
            1,
            "the repository file still contributes exactly its own server"
        );
        assert!(loaded.find_node("web-01").is_some());
        assert!(!loaded.safe_commands.iter().any(|c| c == "sh" || c == "bash" || c == "rm -rf"));
        assert!(!loaded.dangerous_commands.is_empty());
        assert!(!fast.safe_commands.iter().any(|c| c == "sh" || c == "bash" || c == "rm -rf"));
        assert!(!fast.dangerous_commands.is_empty());
        assert!(loaded.from_workspace && fast.from_workspace);
    }

    #[test]
    fn merge_keeps_the_explicit_inventory_and_adds_discovered_hosts() {
        let mut cfg =
            FleetConfig::parse("[[nodes]]\nid = \"node-a\"\nhost = \"192.0.2.10\"\n").unwrap();
        let mut clash = NodeConfig::new("node-a", "198.51.100.99");
        clash.source = NodeSource::SshConfig;
        let mut ssh_host = NodeConfig::new("host-b", "198.51.100.20");
        ssh_host.source = NodeSource::SshConfig;

        cfg.merge_discovered(vec![clash, ssh_host]);

        assert_eq!(cfg.nodes.len(), 2, "a clashing host is not listed twice");
        let explicit = cfg.find_node("node-a").unwrap();
        assert_eq!(explicit.host, "192.0.2.10", "fleet.toml wins on id clashes");
        assert_eq!(explicit.source, NodeSource::Fleet);
        assert_eq!(cfg.find_node("host-b").unwrap().source, NodeSource::SshConfig);
    }

    #[test]
    fn repository_servers_need_approval_but_discovered_ones_do_not() {
        let mut cfg =
            FleetConfig::parse("[[nodes]]\nid = \"repo-node\"\nhost = \"192.0.2.20\"\n").unwrap();
        cfg.from_workspace = true;

        let repo_node = cfg.find_node("repo-node").unwrap().clone();
        assert!(cfg.node_is_untrusted(&repo_node));

        let mut ssh_node = NodeConfig::new("host-b", "198.51.100.20");
        ssh_node.source = NodeSource::SshConfig;
        cfg.merge_discovered(vec![ssh_node]);

        let ssh_node = cfg.find_node("host-b").unwrap();
        assert!(ssh_node.is_user_owned());
        assert!(!cfg.node_is_untrusted(ssh_node));
        assert!(cfg.has_untrusted_nodes());
    }

    #[test]
    fn target_string_brackets_ipv6_literals() {
        let mut v4 = NodeConfig::new("a", "192.0.2.10");
        v4.user = "deploy".to_string();
        assert_eq!(v4.target_string(), "deploy@192.0.2.10:22");

        let mut v6 = NodeConfig::new("b", "2001:db8::1");
        v6.user = "deploy".to_string();
        assert_eq!(v6.target_string(), "deploy@[2001:db8::1]:22");

        let mut already = NodeConfig::new("c", "[2001:db8::2]");
        already.user = "deploy".to_string();
        assert_eq!(already.target_string(), "deploy@[2001:db8::2]:22");
    }

    #[test]
    fn saving_never_persists_discovered_hosts() {
        let dir = std::env::temp_dir().join(format!("corex_fleet_save_src_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut cfg =
            FleetConfig::parse("[[nodes]]\nid = \"node-a\"\nhost = \"192.0.2.10\"\n").unwrap();
        let mut ssh_node = NodeConfig::new("host-b", "198.51.100.20");
        ssh_node.source = NodeSource::SshConfig;
        cfg.merge_discovered(vec![ssh_node]);

        let path = cfg.save(Some(&dir)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(written.contains("node-a"));
        assert!(
            !written.contains("host-b"),
            "hosts discovered in ~/.ssh/config stay there: {written}"
        );
    }
}
