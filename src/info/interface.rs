//! 网络接口信息模块（公共结构 + 分类逻辑）。

use serde::Serialize;

/// 网络接口信息
#[derive(Debug, Clone, Serialize)]
pub struct InterfaceInfo {
    pub name: String,
    pub mac: String,
    pub ipv4: String,
    pub status: String,
    pub description: String,
    pub metric: u32,
    /// 接口类型（英文标识，供 JSON 使用）
    pub iftype: String,
    /// 是否为虚拟网卡
    pub is_virtual: bool,
    /// 是否为出口
    pub is_egress: bool,
    /// 是否为备用默认路由
    pub is_backup: bool,
}

/// 接口类型分类
pub enum IfaceType {
    Loopback,
    Ethernet,
    Wireless,
    MihomoTun,
    ClashTun,
    Wireguard,
    Openvpn,
    Virtualbox,
    Vmware,
    Hyperv,
    Docker,
    TunTap,
    Other,
}

impl IfaceType {
    /// 英文标识（用于 JSON）
    pub fn to_id(&self) -> String {
        match self {
            IfaceType::Loopback => "loopback",
            IfaceType::Ethernet => "ethernet",
            IfaceType::Wireless => "wireless",
            IfaceType::MihomoTun => "mihomo-tun",
            IfaceType::ClashTun => "clash-tun",
            IfaceType::Wireguard => "wireguard",
            IfaceType::Openvpn => "openvpn",
            IfaceType::Virtualbox => "virtualbox",
            IfaceType::Vmware => "vmware",
            IfaceType::Hyperv => "hyperv",
            IfaceType::Docker => "docker",
            IfaceType::TunTap => "tun-tap",
            IfaceType::Other => "other",
        }
        .to_string()
    }

    /// 显示名称（根据语言，统一走 i18n dict）
    pub fn to_label(&self) -> String {
        let key = match self {
            IfaceType::Loopback => "iface.type_loopback",
            IfaceType::Ethernet => "iface.type_ethernet",
            IfaceType::Wireless => "iface.type_wireless",
            IfaceType::MihomoTun => "iface.type_mihomo",
            IfaceType::ClashTun => "iface.type_clash",
            IfaceType::Wireguard => "iface.type_wireguard",
            IfaceType::Openvpn => "iface.type_openvpn",
            IfaceType::Virtualbox => "iface.type_virtualbox",
            IfaceType::Vmware => "iface.type_vmware",
            IfaceType::Hyperv => "iface.type_hyperv",
            IfaceType::Docker => "iface.type_docker",
            IfaceType::TunTap => "iface.type_tuntap",
            IfaceType::Other => "iface.type_other",
        };
        crate::i18n::t(key)
    }

    pub fn is_virtual(&self) -> bool {
        !matches!(
            self,
            IfaceType::Ethernet | IfaceType::Wireless | IfaceType::Loopback | IfaceType::Other
        )
    }
}

/// 根据描述和名称识别接口类型
pub fn classify_interface(desc: &str, name: &str) -> IfaceType {
    let desc_lower = desc.to_lowercase();
    let name_lower = name.to_lowercase();

    if desc_lower.contains("loopback") || name_lower == "lo" || name_lower == "lo0" {
        IfaceType::Loopback
    } else if desc_lower.contains("mihomo") || name_lower.contains("mihomo") {
        IfaceType::MihomoTun
    } else if desc_lower.contains("clash") || name_lower.contains("clash") {
        IfaceType::ClashTun
    } else if desc_lower.contains("wireguard") || name_lower.contains("wg") {
        IfaceType::Wireguard
    } else if desc_lower.contains("openvpn") {
        IfaceType::Openvpn
    } else if desc_lower.contains("virtualbox") || desc_lower.contains("vbox") {
        IfaceType::Virtualbox
    } else if desc_lower.contains("vmware") {
        IfaceType::Vmware
    } else if desc_lower.contains("hyper-v") || desc_lower.contains("vethernet") {
        IfaceType::Hyperv
    } else if desc_lower.contains("docker") {
        IfaceType::Docker
    } else if desc_lower.contains("tun") || desc_lower.contains("tap") {
        IfaceType::TunTap
    } else if desc_lower.contains("wireless")
        || desc_lower.contains("wi-fi")
        || desc_lower.contains("wlan")
    {
        IfaceType::Wireless
    } else if desc_lower.contains("ethernet")
        || desc_lower.contains("以太网")
        || desc_lower.contains("pcie")
        || name_lower.starts_with("eth")
        || name_lower.starts_with("en")
    {
        IfaceType::Ethernet
    } else {
        IfaceType::Other
    }
}
