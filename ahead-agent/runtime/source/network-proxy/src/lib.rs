#![deny(clippy::print_stdout, clippy::print_stderr)]

mod config;
mod disabled;
mod environment_policy;
mod remote_config;
mod request_disconnect;

mod mitm_hook {
    pub use crate::disabled::MitmHookConfig;
}

pub use config::{
    NetworkDomainPermission, NetworkDomainPermissionEntry, NetworkDomainPermissions, NetworkMode,
    NetworkProxyConfig, NetworkUnixSocketPermission, NetworkUnixSocketPermissions,
    host_and_port_from_network_addr, managed_proxy_ports,
};
pub use disabled::*;
pub use environment_policy::EnvironmentNetworkPolicy;
pub use remote_config::{RemoteNetworkProxyConfig, RemoteNetworkProxyLaunchConfig};
pub use request_disconnect::NetworkRequestDisconnect;
