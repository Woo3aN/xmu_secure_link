use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use crate::{api::SecureLinkApi, crypto};

/// Everything the in-process OpenVPN3 client needs: the profile text, the
/// credentials, and the SecureLink `UV_*` peer-info pairs (the OpenVPN3
/// equivalent of 2.x `push-peer-info` + `setenv UV_*`).
#[derive(Debug, Clone)]
pub struct GeneratedConfig {
    pub profile: String,
    pub username: String,
    pub password: String,
    pub peer_info: Vec<(String, String)>,
    pub remotes: Vec<(String, u16)>,
}

pub fn generate(api: &mut SecureLinkApi, vpn_config: &Value) -> Result<GeneratedConfig> {
    let network_config = vpn_config
        .pointer("/content/networkConfig")
        .or_else(|| vpn_config.pointer("/content/NetworkConfig"))
        .context("networkConfig missing")?;
    let client_conf = network_config
        .get("clientConf")
        .or_else(|| network_config.get("ClientConf"))
        .and_then(Value::as_str)
        .context("networkConfig.clientConf missing")?;
    let decoded = urlencoding::decode(client_conf)?.into_owned();
    let mut lines = Vec::new();
    let mut in_cert = false;

    for line in decoded.split('\n') {
        let normalized = if !in_cert || line.contains("BEGIN") || line.contains("END") {
            line.replace('+', " ")
        } else {
            line.to_string()
        };
        let normalized_trimmed = normalized.trim();
        if matches!(normalized_trimmed, "auth-user-pass" | "auth-nocache") {
            continue;
        }
        if normalized_trimmed.starts_with("dev-node ") {
            continue;
        }
        if normalized_trimmed.starts_with("fragment ") || normalized_trimmed == "mssfix 0" {
            continue;
        }
        if normalized_trimmed.starts_with("enable-confusion-proto")
            || normalized_trimmed.starts_with("ignore-unknown-option enable-confusion-proto")
        {
            continue;
        }
        if normalized_trimmed.starts_with("<ca>") {
            in_cert = true;
        } else if normalized_trimmed.starts_with("</ca>") {
            in_cert = false;
        }
        lines.push(normalized);
    }

    let remotes = collect_remote_targets(network_config);
    if remotes.is_empty() {
        return Err(anyhow!("No VPN remotes found in networkConfig"));
    }
    for (host, port) in &remotes {
        lines.push(format!("remote {host} {port}"));
    }

    let access_token = api.access_token()?.to_string();
    let claims = crypto::decode_jwt_payload(&access_token)?;
    let username = claims
        .get("username")
        .and_then(Value::as_str)
        .context("JWT username missing")?
        .to_string();
    let user_id = json_scalar_to_string(claims.get("userId")).unwrap_or_default();
    let device_id = json_scalar_to_string(claims.get("deviceId")).unwrap_or_default();
    let password = crypto::build_management_password(&username);
    let sl_server_type = api.sl_server_type()?;
    let server_ip = remotes
        .iter()
        .find(|(host, _)| is_ipv4(host))
        .map(|(host, _)| host.as_str())
        .unwrap_or(&remotes[0].0);

    lines.push("auth-user-pass".to_string());
    lines.push("data-ciphers AES-128-GCM:AES-256-GCM:CHACHA20-POLY1305:AES-256-CBC".to_string());
    lines.push("ignore-unknown-option app ctrl".to_string());
    // Required: OpenVPN3 only transmits Config.peerInfo (the UV_* values below)
    // when the profile carries `push-peer-info`. SecureLink auth needs them.
    lines.push("push-peer-info".to_string());

    // SecureLink's server-side auth reads these from peer-info. Under 2.x they
    // were emitted as `push-peer-info` + `setenv UV_*`; OpenVPN3 carries them
    // through Config.peerInfo instead.
    let peer_info = vec![
        ("UV_CODE".to_string(), access_token.clone()),
        ("UV_ENCPASS".to_string(), "1".to_string()),
        ("UV_AUTH_TYPE".to_string(), "2".to_string()),
        ("UV_SL_VERSION".to_string(), "3.8.1".to_string()),
        ("UV_VERSION".to_string(), "3.8.1".to_string()),
        ("UV_LOCALE".to_string(), "zh_CN".to_string()),
        ("UV_SYSTEM_NAME".to_string(), "V2luZG93cyAxMA==".to_string()),
        (
            "UV_SYSTEM_MODEL".to_string(),
            "V2luZG93cyAxMA==".to_string(),
        ),
        ("UV_SYSTEM_BITS".to_string(), "NjQ=".to_string()),
        ("UV_DEVICE_MODEL".to_string(), "eDg2XzY0".to_string()),
        ("UV_DEVICE_NAME".to_string(), "UEM=".to_string()),
        ("UV_USERID".to_string(), user_id),
        ("UV_DEVICEID".to_string(), device_id),
        ("UV_SL_SERVERTYPE".to_string(), sl_server_type),
        (
            "UV_EXTRA_PARAM_1".to_string(),
            crypto::b64_json(json!({"system": 0})),
        ),
        (
            "UV_EXTRA_PARAM_2".to_string(),
            crypto::b64_json(json!({"authType": 5})),
        ),
        ("UV_SERVER_IP".to_string(), server_ip.to_string()),
    ];

    Ok(GeneratedConfig {
        profile: lines.join("\n"),
        username,
        password,
        peer_info,
        remotes,
    })
}

fn collect_remote_targets(network_config: &Value) -> Vec<(String, u16)> {
    let mut targets = Vec::new();
    for key in ["priorServers", "alternateServers"] {
        if let Some(items) = network_config.get(key).and_then(Value::as_array) {
            for item in items {
                let Some(host) = item
                    .get("ip")
                    .or_else(|| item.get("host"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                add_target(&mut targets, host, item.get("ports"));
            }
        }
    }
    if let Some(appa) = network_config.get("appaAccConf")
        && let Some(host) = appa.get("host").and_then(Value::as_str)
    {
        add_target(&mut targets, host, appa.get("ports"));
    }
    targets
}

fn add_target(targets: &mut Vec<(String, u16)>, host: &str, ports: Option<&Value>) {
    if let Some(port_items) = ports.and_then(Value::as_array) {
        for port in port_items {
            if let Some(port) = port.as_u64().and_then(|v| u16::try_from(v).ok()) {
                push_unique(targets, host, port);
            }
        }
    } else {
        push_unique(targets, host, 10000);
    }
}

fn push_unique(targets: &mut Vec<(String, u16)>, host: &str, port: u16) {
    let entry = (host.to_string(), port);
    if !targets.contains(&entry) {
        targets.push(entry);
    }
}

fn is_ipv4(host: &str) -> bool {
    host.split('.').count() == 4 && host.split('.').all(|p| p.parse::<u8>().is_ok())
}

fn json_scalar_to_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
