use std::os::windows::process::CommandExt;
use std::process::Command;

use anyhow::{bail, Context, Result};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const TCP_RULE: &str = "PeerPrint TCP";
const MDNS_RULE: &str = "PeerPrint mDNS";

fn run_netsh(args: &[&str]) -> Result<(bool, String)> {
    let output = Command::new("netsh.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("running netsh.exe")?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.success(), text))
}

pub fn rule_exists(name: &str) -> bool {
    run_netsh(&["advfirewall", "firewall", "show", "rule", &format!("name={name}")])
        .map(|(ok, _)| ok)
        .unwrap_or(false)
}

fn delete_rule(name: &str) -> Result<()> {
    let (ok, output) =
        run_netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={name}")])?;
    if !ok {
        tracing::debug!("firewall rule delete {name}: {output}");
    }
    Ok(())
}

pub fn ensure_rules(tcp_port: u16) -> Result<()> {
    delete_rule(TCP_RULE)?;
    delete_rule(MDNS_RULE)?;

    let tcp_port = tcp_port.to_string();
    let (ok, output) = run_netsh(&[
        "advfirewall",
        "firewall",
        "add",
        "rule",
        &format!("name={TCP_RULE}"),
        "dir=in",
        "action=allow",
        "protocol=TCP",
        &format!("localport={tcp_port}"),
        "profile=private",
        "remoteip=localsubnet",
    ])?;
    if !ok {
        bail!("adding TCP firewall rule failed: {output}");
    }

    let (ok, output) = run_netsh(&[
        "advfirewall",
        "firewall",
        "add",
        "rule",
        &format!("name={MDNS_RULE}"),
        "dir=in",
        "action=allow",
        "protocol=UDP",
        "localport=5353",
        "profile=private",
        "remoteip=localsubnet",
    ])?;
    if !ok {
        bail!("adding mDNS firewall rule failed: {output}");
    }
    Ok(())
}

pub fn remove_rules() -> Result<()> {
    delete_rule(TCP_RULE)?;
    delete_rule(MDNS_RULE)?;
    Ok(())
}

#[allow(dead_code)]
pub fn has_rules() -> bool {
    rule_exists(TCP_RULE) && rule_exists(MDNS_RULE)
}
