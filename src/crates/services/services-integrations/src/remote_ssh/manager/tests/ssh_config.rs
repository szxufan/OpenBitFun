use super::super::*;

#[test]
#[cfg(feature = "ssh_config")]
fn ssh_config_fallback_accepts_utf8_bom_and_defaults_to_agent() {
    let content = crate::remote_ssh::ssh_config_loader::strip_utf8_bom(
        "\u{feff}Host 跳板\n  HostName jump.example.com\n  User 构建\n".to_string(),
    );
    let entries = parse_ssh_config_manually(&content);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].host, "跳板");
    assert_eq!(entries[0].user.as_deref(), Some("构建"));
    assert_eq!(entries[0].agent, Some(true));
}

#[test]
fn parses_proxy_jump_alias_user_port_and_ipv6() {
    assert_eq!(
        parse_proxy_jump_token("jump-a").unwrap(),
        (None, "jump-a".to_string(), None)
    );
    assert_eq!(
        parse_proxy_jump_token("ops@jump.example:2222").unwrap(),
        (
            Some("ops".to_string()),
            "jump.example".to_string(),
            Some(2222)
        )
    );
    assert_eq!(
        parse_proxy_jump_token("root@[2001:db8::10]:2200").unwrap(),
        (
            Some("root".to_string()),
            "2001:db8::10".to_string(),
            Some(2200)
        )
    );
}

/// Coder's `config-ssh` output: a multi-word ProxyCommand followed by
/// more keys in the same block, plus a catch-all block below it. This
/// config has a Host before every directive, which is exactly the shape
/// the old `ssh_config` 0.1 path parsed "successfully" while pairing
/// `ProxyCommand` with `/path/coder` and shifting every later key.
#[cfg(feature = "ssh_config")]
const CODER_STYLE_CONFIG: &str = "\
Host coder.*
  ConnectTimeout 0
  ProxyCommand /usr/local/bin/coder --global-config \"/Users/me/Library/Application Support/coderv2\" ssh --stdio --ssh-host-prefix coder. %h
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  User coder
  LogLevel ERROR

Host coder.dev
  HostName ignored-because-proxy-resolves.example

Host plain
  HostName plain.example
  Port 2222
  ProxyCommand none

Host *
  User fallback
  ProxyCommand /usr/bin/nc %h %p
";

#[test]
#[cfg(feature = "ssh_config")]
fn get_ssh_config_resolves_multi_word_proxy_command_without_shifting_keys() {
    let result = SSHConnectionManager::lookup_ssh_config_in(CODER_STYLE_CONFIG, "coder.dev");
    assert!(result.found);
    let entry = result.config.unwrap();
    assert_eq!(
            entry.proxy_command.as_deref(),
            Some(
                "/usr/local/bin/coder --global-config \"/Users/me/Library/Application Support/coderv2\" ssh --stdio --ssh-host-prefix coder. %h"
            )
        );
    // Keys after ProxyCommand keep their own values, and the first
    // obtained value wins over `Host *`.
    assert_eq!(entry.user.as_deref(), Some("coder"));
    assert_eq!(
        entry.hostname.as_deref(),
        Some("ignored-because-proxy-resolves.example")
    );
    assert_eq!(entry.port, None);
    assert_eq!(entry.agent, None);

    // `ProxyCommand none` in a specific block disables the catch-all.
    let plain = SSHConnectionManager::lookup_ssh_config_in(CODER_STYLE_CONFIG, "plain")
        .config
        .unwrap();
    assert_eq!(plain.proxy_command, None);
    assert_eq!(plain.port, Some(2222));
    assert_eq!(plain.user.as_deref(), Some("fallback"));

    // Wildcard matching applies to aliases not listed anywhere.
    let other = SSHConnectionManager::lookup_ssh_config_in(CODER_STYLE_CONFIG, "elsewhere")
        .config
        .unwrap();
    assert_eq!(other.proxy_command.as_deref(), Some("/usr/bin/nc %h %p"));
}

#[test]
#[cfg(feature = "ssh_config")]
fn list_ssh_config_hosts_resolves_each_alias_with_openssh_semantics() {
    let hosts = SSHConnectionManager::list_ssh_config_hosts_in(CODER_STYLE_CONFIG);
    let names: Vec<_> = hosts.iter().map(|entry| entry.host.as_str()).collect();
    // Pure pattern blocks name no connectable host.
    assert_eq!(names, vec!["coder.dev", "plain"]);
    let coder = &hosts[0];
    assert!(coder
        .proxy_command
        .as_deref()
        .is_some_and(|command| command.ends_with("--ssh-host-prefix coder. %h")));
    assert_eq!(coder.user.as_deref(), Some("coder"));
    assert_eq!(hosts[1].proxy_command, None);
    assert_eq!(hosts[1].port, Some(2222));
}

#[test]
#[cfg(feature = "ssh_config")]
fn identity_file_prefers_the_block_naming_the_host() {
    let content = "Host *\n  IdentityFile ~/.ssh/default\nHost foo # work box\n  IdentityFile /keys/foo-1\n  IdentityFile /keys/foo-2\n  CertificateFile /keys/foo-cert.pub\n";
    let entry = SSHConnectionManager::lookup_ssh_config_in(content, "foo")
        .config
        .unwrap();
    assert_eq!(entry.identity_file.as_deref(), Some("/keys/foo-1"));
    assert_eq!(
        entry.certificate_file.as_deref(),
        Some("/keys/foo-cert.pub")
    );
    let settings = resolve_host_settings(content, "foo");
    let files: Vec<_> = settings
        .all("identityfile")
        .iter()
        .map(|value| value.value.as_str())
        .collect();
    assert_eq!(files, vec!["~/.ssh/default", "/keys/foo-1", "/keys/foo-2"]);
    // Hosts without a specific block still get the default.
    let other = SSHConnectionManager::lookup_ssh_config_in(content, "bar")
        .config
        .unwrap();
    assert!(other
        .identity_file
        .as_deref()
        .is_some_and(|file| file.ends_with(".ssh/default")));
    // The trailing comment on the Host line is not a pattern.
    let hosts = SSHConnectionManager::list_ssh_config_hosts_in(content);
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].host, "foo");
}

#[test]
#[cfg(feature = "ssh_config")]
fn proxy_jump_and_proxy_command_first_obtained_wins() {
    let content = "Host target\n  ProxyJump bastion\nHost *\n  ProxyCommand nc %h %p\n";
    let entry = SSHConnectionManager::lookup_ssh_config_in(content, "target")
        .config
        .unwrap();
    assert_eq!(entry.proxy_jump.as_deref(), Some("bastion"));
    assert_eq!(entry.proxy_command, None);

    let content = "Host target\n  ProxyCommand nc %h %p\nHost *\n  ProxyJump bastion\n";
    let entry = SSHConnectionManager::lookup_ssh_config_in(content, "target")
        .config
        .unwrap();
    assert_eq!(entry.proxy_jump, None);
    assert_eq!(entry.proxy_command.as_deref(), Some("nc %h %p"));

    // `ProxyCommand none` first still suppresses a later ProxyJump.
    let content = "Host target\n  ProxyCommand none\nHost *\n  ProxyJump bastion\n";
    let entry = SSHConnectionManager::lookup_ssh_config_in(content, "target")
        .config
        .unwrap();
    assert_eq!(entry.proxy_jump, None);
    assert_eq!(entry.proxy_command, None);
}

#[test]
#[cfg(feature = "ssh_config")]
fn wildcard_only_settings_still_count_as_found() {
    let result =
        SSHConnectionManager::lookup_ssh_config_in("Host *\n  AddKeysToAgent yes\n", "anything");
    assert!(result.found);
    assert_eq!(result.config.unwrap().agent, Some(true));
    assert!(
        !SSHConnectionManager::lookup_ssh_config_in("Host other\n  User x\n", "anything").found
    );
}

#[test]
#[cfg(feature = "ssh_config")]
fn list_ssh_config_hosts_deduplicates_repeated_aliases() {
    let hosts = SSHConnectionManager::list_ssh_config_hosts_in(
        "Host a\n  User first\nHost a\n  User second\nMatch host a\n  Port 1\n",
    );
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].user.as_deref(), Some("first"));
    assert_eq!(hosts[0].port, None);
}

#[test]
fn parses_proxy_command_as_a_whole_line_value() {
    let hosts = parse_ssh_config_manually(
            "Host coder-x\n  ProxyCommand \"C:\\tools\\coder.exe\" ssh --stdio %h\n  IdentityFile ~/.ssh/id_ed25519\n",
        );
    assert_eq!(hosts.len(), 1);
    // The full remainder of the line survives, inner quotes included.
    assert_eq!(
        hosts[0].proxy_command.as_deref(),
        Some("\"C:\\tools\\coder.exe\" ssh --stdio %h")
    );

    // Kept verbatim like OpenSSH: stripping the outer quotes of
    // `"a b" ssh "%h"` would corrupt it.
    let hosts = parse_ssh_config_manually("Host quoted\n  ProxyCommand \"a b\" ssh \"%h\"\n");
    assert_eq!(
        hosts[0].proxy_command.as_deref(),
        Some("\"a b\" ssh \"%h\"")
    );

    let hosts = parse_ssh_config_manually("Host plain\n  HostName plain.example\n");
    assert_eq!(hosts[0].proxy_command, None);
}
