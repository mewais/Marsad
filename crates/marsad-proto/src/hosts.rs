//! `hosts.*` (hub): the machines sessions run on, and their link states.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wasit_control::{HostId, LinkState};

/// How the hub reaches a host: "a command whose stdio is the link".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostKind {
    /// The hub's own machine.
    Local,
    /// OpenSSH (`ssh` with `BatchMode=yes` and keepalives; Unix and Windows OpenSSH servers).
    Ssh {
        /// `~/.ssh/config` alias or `user@host`.
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_file: Option<String>,
        /// Extra `-o` options, e.g. `ProxyJump=bastion`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
    },
    /// A WSL distro (only from a Windows hub), via `wsl.exe -d <distro>`.
    Wsl { distro: String },
}

/// What a host's daemon runs on, as found by the last provisioning probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostPlatform {
    /// `linux`, `macos`, `windows`.
    pub os: String,
    pub arch: String,
    pub target: String,
}

/// A host the hub knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostEntry {
    pub id: HostId,
    pub name: String,
    pub kind: HostKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<HostPlatform>,
    pub created_at_ms: u64,
    /// The host needs the user at login: the last connect that reached it asked the user for
    /// something other than a host-key confirmation (a password, a one-time code, a key
    /// passphrase; see [`PromptKind::needs_user`]). Such a link never reconnects on its own,
    /// since every sign-in would spend a code or fire a push: when no shared connection to it is
    /// open (`shared_connection`; on a hub without shared connections always), after a drop and
    /// at the hub's start it waits in `auth_needed` (`held`) for an explicit `hosts.reconnect`.
    /// Learned by
    /// the hub from each successful sign-in, and un-learned by one that asked for nothing.
    #[serde(default)]
    pub interactive_auth: bool,
    /// The host's files are directly reachable from the hub's machine (the GUI machine): true
    /// for the local host, and for a WSL host when the hub runs on Windows. Derived by the hub
    /// ([`HostEntry::derive_local_files`]); the UI decides open / reveal / download from it.
    #[serde(default)]
    pub local_files: bool,
    /// The hub keeps one authenticated ssh connection open to this host and every link of it
    /// rides that connection (Linux and macOS hubs, ssh hosts): the user signs in once, the
    /// connection survives the hub's restarts, and `hosts.signout` ends it. Derived by the hub
    /// ([`HostEntry::derive_local_files`]); a Windows hub's ssh client cannot share a
    /// connection, so there it is `false` and the UI offers no sign-out.
    #[serde(default)]
    pub shared_connection: bool,
    /// The user explicitly signed out of this host's shared connection: it is not retried until
    /// they ask to sign in again (`hosts.reconnect` clears it). Stored with the host so a hub
    /// restart keeps it parked.
    #[serde(default)]
    pub signed_out: bool,
    /// Key of the machine the host's daemon runs on (from its hello; the hub's own for the
    /// local host). Two hosts with the same key are the same machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

impl HostEntry {
    /// Whether files of a host of this kind are directly reachable from a hub running on
    /// Windows (`hub_windows`) or elsewhere.
    pub fn files_reachable(kind: &HostKind, hub_windows: bool) -> bool {
        match kind {
            HostKind::Local => true,
            HostKind::Wsl { .. } => hub_windows,
            HostKind::Ssh { .. } => false,
        }
    }

    /// Other hosts in `all` that run on the same machine as `self` (same machine key).
    pub fn same_machine<'a>(&self, all: &'a [HostEntry]) -> Vec<&'a HostEntry> {
        match &self.machine {
            None => Vec::new(),
            Some(m) => all
                .iter()
                .filter(|h| h.id != self.id && h.machine.as_deref() == Some(m))
                .collect(),
        }
    }

    /// Whether a hub running on Windows (`hub_windows`) or elsewhere keeps a shared ssh
    /// connection for a host of this kind.
    pub fn connection_shared(kind: &HostKind, hub_windows: bool) -> bool {
        matches!(kind, HostKind::Ssh { .. }) && !hub_windows
    }

    /// Set what is derived from the machine this hub runs on: `local_files` and
    /// `shared_connection`.
    pub fn derive_local_files(&mut self) {
        self.local_files = Self::files_reachable(&self.kind, cfg!(windows));
        self.shared_connection = Self::connection_shared(&self.kind, cfg!(windows));
    }
}

/// What an authentication prompt asks for (a guess from its text, for presentation only,
/// except that [`PromptKind::Confirm`] is exactly ssh's host-key confirmation, which is what
/// [`PromptKind::needs_user`] rests on; the prompt text itself is shown verbatim).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// A password or key passphrase (mask the input).
    Secret,
    /// A one-time code.
    Code,
    /// ssh's own question whether to trust a host's key (never a question a site asks).
    Confirm,
    Info,
}

impl PromptKind {
    /// ssh's own host-key confirmation: the question about an unknown (or differently
    /// addressed) host key, and its repeat after an answer that was none of yes, no or the
    /// fingerprint. OpenSSH writes these words itself (they are not translated and no site can
    /// set them), so they are the one thing in a prompt that can be relied on. ssh gives the
    /// askpass helper no other sign of what it asks: `SSH_ASKPASS_PROMPT` is not set for this
    /// question (measured with OpenSSH 9.6; it hints only ssh-agent's confirmations and
    /// security-key notices), nor for a password or a passphrase.
    pub fn is_host_key_confirmation(prompt: &str) -> bool {
        prompt.contains("continue connecting (yes/no") || prompt.starts_with("Please type 'yes'")
    }

    pub fn guess(prompt: &str) -> PromptKind {
        let p = prompt.to_ascii_lowercase();
        if Self::is_host_key_confirmation(prompt) {
            PromptKind::Confirm
        } else if [
            "verification code",
            "one-time",
            "otp",
            "token",
            "passcode",
            "code:",
            "duo",
        ]
        .iter()
        .any(|n| p.contains(n))
        {
            PromptKind::Code
        } else if p.contains("password") || p.contains("passphrase") || p.trim_end().ends_with(':')
        {
            PromptKind::Secret
        } else {
            PromptKind::Info
        }
    }

    /// THE rule for "this host needs the user at login": a prompt of this kind asks the user
    /// for something (a password, a passphrase, a one-time code, a push approval or anything
    /// else a site asks), so a connect that raised one cannot be repeated without them. Only
    /// the host-key confirmation does not count: it is asked once, then ssh remembers the
    /// answer. What cannot be told apart (and need not be): a password, a passphrase, a code
    /// and a site's own question are all entries to ssh and differ only in words. A push
    /// approval that ssh asks no entry for (only prints) never reaches the helper at all, so
    /// it cannot be seen from here.
    pub fn needs_user(self) -> bool {
        !matches!(self, PromptKind::Confirm)
    }
}

/// Why a sign-in prompt is a repeat: it belongs to a second connection within the same
/// sign-in, which the hub makes on its own after the user's one Connect, so the user is asked
/// again (a second approval, a second code) and is told why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SignInRepeat {
    /// The Marsad helper on the host runs another build (or cannot be used as it is) and is
    /// being replaced and restarted over a new connection.
    HelperRestart,
    /// The helper's daemon on the host knew itself under another host id and is being
    /// restarted under this host's.
    IdRestart,
    /// The host turned out to be a Windows machine: the connection is made again the way
    /// Windows hosts need.
    WindowsShell,
}

/// A pending authentication prompt of a host's link (answer with `hosts.auth_answer`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AuthPrompt {
    pub id: u64,
    pub host: HostId,
    /// The prompt exactly as ssh printed it.
    pub prompt: String,
    pub kind: PromptKind,
    /// 1 the first time this prompt is asked in a connect attempt, 2 when it comes again
    /// (the answer was wrong), … A different prompt (a second factor) starts at 1.
    pub attempt: u32,
    /// When the prompt gives up (ms since the Unix epoch); the link then goes `auth_needed`.
    pub expires_at_ms: u64,
    /// Set when this prompt comes from a second connection within one sign-in (the user was
    /// already asked on the first): why the hub connected again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub repeat: Option<SignInRepeat>,
}

/// `hosts.auth_answer`. The answer is passed to ssh and never logged or stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AuthAnswer {
    pub host: HostId,
    pub id: u64,
    pub answer: String,
}

/// `hosts.auth_cancel`: give up the prompt (the connect attempt fails as `auth_needed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AuthCancel {
    pub host: HostId,
    pub id: u64,
}

/// `hosts.askpass` (internal: sent by `marsad-agent askpass`, the `SSH_ASKPASS` helper).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskpassRequest {
    pub host: HostId,
    /// The connect attempt whose ssh runs the helper (the number the hub put in its
    /// environment): a prompt is only taken for the attempt in progress, so a helper left over
    /// from an ended attempt can never put a prompt before the user or answer one of a newer
    /// attempt. `0`: no attempt (a host nobody is connecting to).
    pub attempt: u64,
    pub prompt: String,
}

/// Result of `hosts.askpass`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskpassReply {
    pub answer: String,
}

/// `hosts.add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostDraft {
    pub name: String,
    pub kind: HostKind,
    /// Requested id (`None` = derived from the name). Must be unique.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<HostId>,
}

/// Names one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostRef {
    pub id: HostId,
}

/// Result of `hosts.list` (the local host first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostList {
    pub hosts: Vec<HostEntry>,
}

/// One `Host` block of `~/.ssh/config` that can become a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SshConfigHost {
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    /// A host with this alias as its ssh target is already configured.
    pub already_added: bool,
}

/// Result of `hosts.import_suggestions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ImportSuggestions {
    pub hosts: Vec<SshConfigHost>,
    /// The file that was read (absent = none found).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// One installed WSL distribution (from `wsl.exe -l -v`, or `-l -q` on WSL versions without
/// `-v`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WslDistro {
    pub name: String,
    /// WSL's default distribution (`wsl.exe` without `-d`).
    pub default: bool,
    /// Running right now (`false` also when unknown).
    pub running: bool,
    /// WSL version (1 or 2) when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
}

/// Result of `hosts.wsl_distros`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WslDistros {
    /// WSL hosts can be added: the hub runs on Windows and `wsl.exe` exists.
    pub available: bool,
    pub distros: Vec<WslDistro>,
    /// Why the list is empty or WSL is unavailable, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// A host's environment settings as the UI edits them (`hosts.environment.get` / `.set`):
/// setup commands, variables, and an agent command per driver (ARCHITECTURE.md §3.5 of
/// Wasit). Applied to sessions, terminals and git started after they reach the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostEnvSettings {
    pub host: HostId,
    #[serde(default)]
    pub setup: String,
    #[serde(default)]
    pub vars: Vec<HostVar>,
    #[serde(default)]
    pub commands: BTreeMap<wasit_core::DriverId, String>,
    /// Changes on every save (`hosts.environment.set` ignores it); a host whose
    /// `HostEnvironment.revision` equals it has these settings.
    #[serde(default)]
    pub revision: String,
}

impl HostEnvSettings {
    /// No setup, variables or commands.
    pub fn empty(host: HostId) -> Self {
        HostEnvSettings {
            host,
            setup: String::new(),
            vars: Vec::new(),
            commands: BTreeMap::new(),
            revision: String::new(),
        }
    }
}

/// One variable of [`HostEnvSettings`]. A secret's value never leaves the hub towards the UI:
/// `get` answers `value: None` for it, and `set` with `value: None` keeps the stored one.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostVar {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default)]
    pub secret: bool,
}

impl std::fmt::Debug for HostVar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value: &dyn std::fmt::Debug = match (&self.value, self.secret) {
            (Some(_), true) => &"<secret>",
            (v, _) => v,
        };
        f.debug_struct("HostVar")
            .field("name", &self.name)
            .field("value", value)
            .field("secret", &self.secret)
            .finish()
    }
}

/// Result of `hosts.environment.set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostEnvSaved {
    /// As stored (secret values left out).
    pub settings: HostEnvSettings,
    /// The host's environment after applying them, when it could be reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<wasit_runtime::wire::HostEnvironment>,
    /// Why they are not applied yet (the host is not connected, …); they are applied when it
    /// connects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_applied: Option<String>,
}

/// Where the hub keeps secret variable values (result of `hosts.environment.secrets`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SecretsLocation {
    /// The operating system's credential store.
    System {
        /// Its name, e.g. "Windows Credential Manager", "macOS Keychain", "Secret Service".
        store: String,
    },
    /// A file only this user can read (`secrets.json` in the hub's store), because the system
    /// store cannot be used.
    File {
        path: String,
        /// Why not the system store.
        reason: String,
    },
}

/// Payload of `hosts.links`: every remote host's link state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LinkStates {
    pub states: BTreeMap<HostId, LinkState>,
}

/// Parse `~/.ssh/config` text: `Host` blocks with `HostName`, `User`, `Port`, `IdentityFile`.
/// Patterns (`*`, `?`, `!`) are not hosts and are skipped; `Match` blocks are skipped.
pub fn parse_ssh_config(text: &str) -> Vec<SshConfigHost> {
    let mut out: Vec<SshConfigHost> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (kw, val) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (
                k.to_ascii_lowercase(),
                v.trim().trim_start_matches('=').trim(),
            ),
            None => continue,
        };
        let val = val.trim_matches('"');
        match kw.as_str() {
            "host" => {
                current.clear();
                for alias in val.split_whitespace() {
                    if alias.contains(['*', '?', '!']) {
                        continue;
                    }
                    current.push(out.len());
                    out.push(SshConfigHost {
                        alias: alias.to_string(),
                        hostname: None,
                        user: None,
                        port: None,
                        identity_file: None,
                        already_added: false,
                    });
                }
            }
            "match" => current.clear(),
            "hostname" | "user" | "port" | "identityfile" => {
                for &i in &current {
                    let h = &mut out[i];
                    match kw.as_str() {
                        "hostname" => h.hostname = h.hostname.clone().or(Some(val.to_string())),
                        "user" => h.user = h.user.clone().or(Some(val.to_string())),
                        "port" => h.port = h.port.or(val.parse().ok()),
                        _ => h.identity_file = h.identity_file.clone().or(Some(val.to_string())),
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config_blocks() {
        let text = "\
# comment
Host *
  ServerAliveInterval 30
Host box dev-box
  HostName 10.0.0.5
  User me
  Port 2222
  IdentityFile ~/.ssh/id_box
Host *.internal !x
  User nobody
Match host foo
  User ignored
Host=eqhost
  HostName=eq.example
";
        let h = parse_ssh_config(text);
        let aliases: Vec<&str> = h.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["box", "dev-box", "eqhost"]);
        assert_eq!(h[0].hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(h[1].port, Some(2222));
        assert_eq!(h[1].identity_file.as_deref(), Some("~/.ssh/id_box"));
        assert_eq!(h[2].hostname.as_deref(), Some("eq.example"));
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    /// What ssh itself writes when it asks to trust a host's key, as OpenSSH 9.6 does (the
    /// question comes with the fingerprint lines before it; the repeat after a wrong answer is
    /// its own sentence).
    const HOST_KEY: [&str; 4] = [
        "The authenticity of host '[127.0.0.1]:22 ([127.0.0.1]:22)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nThis key is not known by any other names.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ",
        "Are you sure you want to continue connecting (yes/no)? ",
        "Please type 'yes', 'no' or the fingerprint: ",
        "Please type 'yes' or 'no': ",
    ];

    #[test]
    fn only_ssh_own_host_key_confirmation_does_not_need_the_user() {
        for p in HOST_KEY {
            assert_eq!(PromptKind::guess(p), PromptKind::Confirm, "{p}");
            assert!(PromptKind::is_host_key_confirmation(p), "{p}");
            assert!(!PromptKind::guess(p).needs_user(), "{p}");
        }
        // Everything else a user can be asked at login does.
        for (p, kind) in [
            ("user@host's password: ", PromptKind::Secret),
            (
                "Enter passphrase for key '/home/u/.ssh/id_ed25519': ",
                PromptKind::Secret,
            ),
            ("Verification code: ", PromptKind::Code),
            ("Duo passcode or option (1-3): ", PromptKind::Code),
            ("One-time password (OATH) for `u': ", PromptKind::Code),
            (
                "Confirm user presence for key ED25519-SK SHA256:abc",
                PromptKind::Info,
            ),
            // A site's own yes/no question is not ssh's, and is the user's.
            ("Do you accept the terms of use (yes/no)?", PromptKind::Info),
        ] {
            assert_eq!(PromptKind::guess(p), kind, "{p}");
            assert!(PromptKind::guess(p).needs_user(), "{p}");
        }
    }
}

#[cfg(test)]
mod local_files_tests {
    use super::*;

    #[test]
    fn only_local_and_wsl_on_windows_reach_files() {
        let wsl = HostKind::Wsl { distro: "U".into() };
        let ssh = HostKind::Ssh {
            target: "a".into(),
            port: None,
            identity_file: None,
            options: vec![],
        };
        assert!(HostEntry::files_reachable(&HostKind::Local, false));
        assert!(HostEntry::files_reachable(&wsl, true));
        assert!(!HostEntry::files_reachable(&wsl, false));
        assert!(!HostEntry::files_reachable(&ssh, true));
    }
}

#[cfg(test)]
mod shared_connection_tests {
    use super::*;

    /// Only an ssh host on a hub whose ssh client can share a connection (not Windows) has one:
    /// the fact the app reads to offer *Sign out*, decided by the hub and not by the app's OS.
    #[test]
    fn only_an_ssh_host_on_a_hub_that_can_share_has_a_shared_connection() {
        let ssh = HostKind::Ssh {
            target: "me@box".into(),
            port: None,
            identity_file: None,
            options: vec![],
        };
        let wsl = HostKind::Wsl { distro: "d".into() };
        assert!(HostEntry::connection_shared(&ssh, false));
        assert!(!HostEntry::connection_shared(&ssh, true), "a Windows hub");
        assert!(!HostEntry::connection_shared(&wsl, false));
        assert!(!HostEntry::connection_shared(&HostKind::Local, false));
        let mut e = HostEntry {
            id: "h".into(),
            name: "h".into(),
            kind: ssh,
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            local_files: false,
            shared_connection: false,
            signed_out: false,
            machine: None,
        };
        e.derive_local_files();
        assert_eq!(e.shared_connection, !cfg!(windows));
    }
}
