//! The Wi-Fi network this computer is on, for the "share Wi-Fi" QR code.
//!
//! Everything here shells out to the platform's own tooling, with arguments
//! passed as an argv (never through a shell), and is reachable only from the
//! local window — no network route calls into this module.
//!
//! Reading the *name* needs no special rights. Reading the *password* does:
//! macOS shows its own Keychain prompt, which is the point — the user decides,
//! not this app.

use serde::Serialize;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct WifiInfo {
    /// `None` when not on Wi-Fi, or the OS would not say.
    pub ssid: Option<String>,
    /// The `T:` value of a Wi-Fi QR code: `WPA`, `WEP` or `nopass`.
    pub security: String,
}

/// How long the name lookup may take. `system_profiler` is slow (~5 s) but
/// is the one macOS source that does not redact the SSID without Location
/// Services permission.
const INFO_TIMEOUT: Duration = Duration::from_secs(20);
/// The password lookup can sit behind a system prompt the user has to answer.
const PASSWORD_TIMEOUT: Duration = Duration::from_secs(120);

pub async fn current() -> WifiInfo {
    current_impl().await.unwrap_or_else(|| WifiInfo {
        ssid: None,
        security: "WPA".into(),
    })
}

/// The saved password for `ssid`, or an explanation of why there is none.
///
/// The first time, macOS has to be asked — on a Mac that means its own
/// administrator dialog, because Wi-Fi passwords live in the System keychain
/// and only Apple's apps may unlock that with Touch ID. ShareLocal then keeps
/// a copy in the user's login keychain, so every later request is a single
/// Touch ID (or login-password) check with no user name to remember.
pub async fn password(ssid: &str) -> Result<String, String> {
    let ssid = ssid.trim();
    if ssid.is_empty() {
        return Err("Enter the network name first.".into());
    }
    #[cfg(target_os = "macos")]
    if let Some(saved) = mac::remembered(ssid) {
        mac::confirm_owner(&format!("show the Wi-Fi password for “{ssid}”")).await?;
        return Ok(saved);
    }
    let found = password_impl(ssid).await?;
    remember(ssid, &found);
    Ok(found)
}

/// What the password button can promise before it is pressed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PasswordAccess {
    /// ShareLocal already holds a copy, so only Touch ID / the login password
    /// stands between the user and it.
    pub remembered: bool,
    /// The Mac has Touch ID set up.
    pub biometrics: bool,
    /// The account name macOS's administrator dialog expects, for the hint
    /// shown before that dialog appears.
    pub user_name: String,
}

pub fn access(ssid: &str) -> PasswordAccess {
    #[cfg(target_os = "macos")]
    {
        PasswordAccess {
            remembered: mac::remembered(ssid.trim()).is_some(),
            biometrics: mac::has_biometrics(),
            user_name: std::env::var("USER").unwrap_or_default(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ssid;
        PasswordAccess { remembered: false, biometrics: false, user_name: String::new() }
    }
}

/// Keep a copy of a password the user fetched or typed, for next time.
pub fn remember(ssid: &str, password: &str) {
    #[cfg(target_os = "macos")]
    if !ssid.trim().is_empty() && !password.is_empty() {
        mac::remember(ssid.trim(), password);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (ssid, password);
}

pub fn forget(ssid: &str) {
    #[cfg(target_os = "macos")]
    mac::forget(ssid.trim());
    #[cfg(not(target_os = "macos"))]
    let _ = ssid;
}

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    /// The login-keychain item ShareLocal keeps its copies under, one per SSID.
    const SERVICE: &str = "ShareLocal Wi-Fi password";

    pub fn remembered(ssid: &str) -> Option<String> {
        security_framework::passwords::get_generic_password(SERVICE, ssid)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .filter(|p| !p.is_empty())
    }

    pub fn remember(ssid: &str, password: &str) {
        if remembered(ssid).as_deref() == Some(password) {
            return;
        }
        let _ = security_framework::passwords::set_generic_password(SERVICE, ssid, password.as_bytes());
    }

    pub fn forget(ssid: &str) {
        let _ = security_framework::passwords::delete_generic_password(SERVICE, ssid);
    }

    pub fn has_biometrics() -> bool {
        // SAFETY: plain Objective-C calls on a context we own.
        unsafe {
            LAContext::new()
                .canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
                .is_ok()
        }
    }

    /// Touch ID, falling back to the login password — the system sheet that
    /// says "ShareLocal is trying to …". No user name is asked for.
    pub async fn confirm_owner(reason: &str) -> Result<(), String> {
        let reason = reason.to_string();
        tokio::task::spawn_blocking(move || {
            let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
            // SAFETY: the context and block outlive the call; the reply runs
            // once on a system queue and only sends on the channel.
            unsafe {
                let context = LAContext::new();
                let block = RcBlock::new(move |ok: Bool, error: *mut NSError| {
                    let result = if ok.as_bool() {
                        Ok(())
                    } else {
                        let code = if error.is_null() { 0 } else { (*error).code() };
                        Err(match code {
                            // userCancel, systemCancel, appCancel, userFallback
                            -2 | -4 | -9 | -3 => "cancelled".to_string(),
                            _ => "macOS could not confirm it was you.".to_string(),
                        })
                    };
                    let _ = tx.send(result);
                });
                context.evaluatePolicy_localizedReason_reply(
                    LAPolicy::DeviceOwnerAuthentication,
                    &NSString::from_str(&reason),
                    &block,
                );
                rx.recv().unwrap_or_else(|_| Err("macOS did not answer.".into()))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

async fn run(program: &str, args: &[&str], timeout: Duration) -> Option<std::process::Output> {
    let child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(timeout, child).await.ok()?.ok()
}

// ------------------------------------------------------------------ macOS

#[cfg(target_os = "macos")]
async fn current_impl() -> Option<WifiInfo> {
    let out = run("system_profiler", &["SPAirPortDataType", "-json", "-detailLevel", "basic"], INFO_TIMEOUT).await?;
    parse_system_profiler(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(target_os = "macos")]
async fn password_impl(ssid: &str) -> Result<String, String> {
    let out = run(
        "security",
        &["find-generic-password", "-D", "AirPort network password", "-a", ssid, "-w"],
        PASSWORD_TIMEOUT,
    )
    .await
    .ok_or("The Keychain did not answer in time.")?;
    if !out.status.success() {
        return Err(
            "No saved password for that network, or access to the Keychain was declined.".into(),
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\n', '\r']).to_string())
}

/// Pull the associated network out of `system_profiler SPAirPortDataType -json`.
pub fn parse_system_profiler(json: &str) -> Option<WifiInfo> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let interfaces = value
        .get("SPAirPortDataType")?
        .as_array()?
        .iter()
        .filter_map(|entry| entry.get("spairport_airport_interfaces")?.as_array())
        .flatten();
    for iface in interfaces {
        let Some(current) = iface.get("spairport_current_network_information") else {
            continue;
        };
        let Some(name) = current.get("_name").and_then(|n| n.as_str()) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let mode = current
            .get("spairport_security_mode")
            .and_then(|m| m.as_str())
            .unwrap_or("");
        return Some(WifiInfo {
            ssid: Some(name.to_string()),
            security: security_from(mode).into(),
        });
    }
    None
}

// ---------------------------------------------------------------- Windows

#[cfg(target_os = "windows")]
async fn current_impl() -> Option<WifiInfo> {
    let out = run("netsh", &["wlan", "show", "interfaces"], INFO_TIMEOUT).await?;
    parse_netsh_interfaces(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(target_os = "windows")]
async fn password_impl(ssid: &str) -> Result<String, String> {
    let name = format!("name={ssid}");
    let out = run("netsh", &["wlan", "show", "profile", &name, "key=clear"], PASSWORD_TIMEOUT)
        .await
        .ok_or("Windows did not answer in time.")?;
    let text = String::from_utf8_lossy(&out.stdout);
    netsh_field(&text, "Key Content")
        .ok_or_else(|| "No saved password for that network.".to_string())
}

/// `netsh` prints `    Name   : value` lines. English labels only; on other
/// locales the lookup simply comes back empty and the user types it in.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn netsh_field(text: &str, label: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == label).then(|| value.trim().to_string())
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn parse_netsh_interfaces(text: &str) -> Option<WifiInfo> {
    let ssid = netsh_field(text, "SSID").filter(|s| !s.is_empty())?;
    let auth = netsh_field(text, "Authentication").unwrap_or_default();
    Some(WifiInfo {
        ssid: Some(ssid),
        security: security_from(&auth).into(),
    })
}

// ------------------------------------------------------------------ Linux

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn current_impl() -> Option<WifiInfo> {
    let out = run("nmcli", &["-t", "-f", "ACTIVE,SSID,SECURITY", "dev", "wifi"], INFO_TIMEOUT).await?;
    parse_nmcli_wifi(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn password_impl(ssid: &str) -> Result<String, String> {
    let out = run(
        "nmcli",
        &["-s", "-g", "802-11-wireless-security.psk", "connection", "show", "id", ssid],
        PASSWORD_TIMEOUT,
    )
    .await
    .ok_or("NetworkManager did not answer in time.")?;
    let pw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || pw.is_empty() {
        return Err("No saved password for that network.".into());
    }
    Ok(pw)
}

/// `nmcli -t` separates fields with `:` and escapes literal colons as `\:`.
#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
pub fn parse_nmcli_wifi(text: &str) -> Option<WifiInfo> {
    text.lines().find_map(|line| {
        let fields = split_nmcli(line);
        if fields.first().map(String::as_str) != Some("yes") {
            return None;
        }
        let ssid = fields.get(1).filter(|s| !s.is_empty())?.clone();
        let security = fields.get(2).cloned().unwrap_or_default();
        Some(WifiInfo {
            ssid: Some(ssid),
            security: security_from(&security).into(),
        })
    })
}

#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
fn split_nmcli(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    fields.last_mut().unwrap().push(next);
                }
            }
            ':' => fields.push(String::new()),
            _ => fields.last_mut().unwrap().push(c),
        }
    }
    fields
}

/// Map whatever the OS calls the security mode onto a QR `T:` value. WPA2 and
/// WPA3 both use `WPA`, which every phone camera understands.
fn security_from(mode: &str) -> &'static str {
    let mode = mode.to_ascii_lowercase();
    if mode.contains("wep") {
        "WEP"
    } else if mode.is_empty()
        || mode.contains("none")
        || mode.contains("open")
        || mode == "--"
        || mode.ends_with("mode_none")
    {
        "nopass"
    } else {
        "WPA"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_associated_network_from_system_profiler() {
        let json = r#"{"SPAirPortDataType":[{"spairport_airport_interfaces":[
            {"_name":"en0","spairport_current_network_information":
                {"_name":"Home Net","spairport_security_mode":"spairport_security_mode_wpa2_personal"}},
            {"_name":"awdl0"}]}]}"#;
        assert_eq!(
            parse_system_profiler(json),
            Some(WifiInfo { ssid: Some("Home Net".into()), security: "WPA".into() })
        );
    }

    #[test]
    fn no_network_when_not_associated() {
        let json = r#"{"SPAirPortDataType":[{"spairport_airport_interfaces":[{"_name":"en0"}]}]}"#;
        assert_eq!(parse_system_profiler(json), None);
        assert_eq!(parse_system_profiler("not json"), None);
    }

    #[test]
    fn reads_netsh_output() {
        let text = "    Name                   : Wi-Fi\n    SSID                   : Cafe: Guest\n    BSSID                  : aa:bb\n    Authentication         : WPA2-Personal\n";
        // `split_once` keeps everything after the first colon, so a colon in
        // the SSID survives.
        assert_eq!(
            parse_netsh_interfaces(text),
            Some(WifiInfo { ssid: Some("Cafe: Guest".into()), security: "WPA".into() })
        );
        assert_eq!(netsh_field("    Key Content            : hunter2\n", "Key Content").as_deref(), Some("hunter2"));
    }

    #[test]
    fn reads_nmcli_output_with_escaped_colons() {
        let text = "no:Other:WPA2\nyes:Lab\\:5G:WPA2 WPA3\n";
        assert_eq!(
            parse_nmcli_wifi(text),
            Some(WifiInfo { ssid: Some("Lab:5G".into()), security: "WPA".into() })
        );
        assert_eq!(parse_nmcli_wifi("yes:Open Cafe:\n").unwrap().security, "nopass");
    }

    #[test]
    fn maps_security_modes() {
        assert_eq!(security_from("spairport_security_mode_wep"), "WEP");
        assert_eq!(security_from("spairport_security_mode_none"), "nopass");
        assert_eq!(security_from("Open"), "nopass");
        assert_eq!(security_from("WPA3 Personal"), "WPA");
    }
}
