use std::path::Path;

const RELAUNCH_INTERVAL: &str = "PT1M";
const RELAUNCH_SINCE: &str = "2020-01-01T00:00:00";

/// Name every install used before tasks were named per user. Task names are
/// machine-wide, so a second account could not install over the first one's.
pub const LEGACY_TASK_NAME: &str = "rtk-proxy";

/// Task name for `user` (`DOMAIN\name`), with every character Task Scheduler
/// or a command line could trip on replaced by `_`.
pub fn task_name(user: Option<&str>) -> String {
    let safe: String = user
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        LEGACY_TASK_NAME.to_string()
    } else {
        format!("{LEGACY_TASK_NAME}-{safe}")
    }
}

/// The account this process runs as, as Windows names it in a task's
/// `<Principal><UserId>`: a SID, `DOMAIN\name`, or a bare local name.
pub struct Account {
    pub sid: Option<String>,
    pub domain: Option<String>,
    pub name: String,
    pub computer: Option<String>,
}

/// The SID from `whoami /user /fo csv /nh` (`"domain\name","S-1-..."`).
pub fn sid_from_whoami(out: &str) -> Option<String> {
    out.split(',')
        .map(|field| field.trim().trim_matches('"'))
        .find(|field| field.starts_with("S-1-"))
        .map(str::to_string)
}

/// `schtasks /Query /XML` writes 8-bit text to a pipe, whatever its XML
/// header says; a UTF-16 answer is decoded too.
pub fn decode_output(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&pair| u16::from_le_bytes(pair))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The `<UserId>` of the task's principal, not the one of its logon trigger.
pub fn principal_user_id(xml: &str) -> Option<String> {
    let start = xml
        .match_indices("<Principal")
        .map(|(at, tag)| at + tag.len())
        .find(|&end| matches!(xml.as_bytes().get(end), Some(b' ' | b'>')))?;
    let block = &xml[start..];
    let block = &block[..block.find("</Principal>")?];
    let value = block.split_once("<UserId>")?.1.split_once("</UserId>")?.0;
    Some(unescape(value.trim()))
}

/// Whether a task whose principal is `user_id` runs as `me`. Anything we
/// cannot tie to `me` belongs to someone else: an elevated terminal sees every
/// account's tasks, and deleting one of theirs is worse than leaving ours.
pub fn owned_by(user_id: &str, me: &Account) -> bool {
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    if user_id.get(..4).is_some_and(|p| same(p, "S-1-")) {
        return me.sid.as_deref().is_some_and(|sid| same(sid, user_id));
    }
    match user_id.split_once('\\') {
        Some((domain, name)) => {
            same(name, &me.name) && me.domain.as_deref().is_some_and(|d| same(d, domain))
        }
        // A bare name is a local account: ours only if we are that local account.
        None => {
            same(user_id, &me.name)
                && matches!(
                    (me.domain.as_deref(), me.computer.as_deref()),
                    (Some(d), Some(c)) if same(d, c)
                )
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn definition(bin: &Path, config_path: &Path, log_path: &Path, user: Option<&str>) -> String {
    let arguments = format!(
        "/C \"set RTK_PROXY_CONFIG={config} && \"{bin}\" start >> \"{log}\" 2>&1\"",
        config = config_path.display(),
        bin = bin.display(),
        log = log_path.display(),
    );
    let user_id = user
        .map(|u| format!("\n      <UserId>{}</UserId>", escape(u)))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>rtk-proxy -- local LLM proxy</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>{user_id}
    </LogonTrigger>
    <TimeTrigger>
      <Repetition>
        <Interval>{RELAUNCH_INTERVAL}</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <StartBoundary>{RELAUNCH_SINCE}</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">{user_id}
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>cmd</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        arguments = escape(&arguments),
    )
}

pub fn encode(xml: &str) -> Vec<u8> {
    std::iter::once(0xFEFFu16)
        .chain(xml.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(user: Option<&str>) -> String {
        definition(
            Path::new(r"C:\Users\a&b\.local\bin\rtk-proxy.exe"),
            Path::new(r"C:\Users\a&b\AppData\Roaming\rtk-proxy\rtk-proxy.toml"),
            Path::new(r"C:\Users\a&b\AppData\Roaming\rtk-proxy\rtk-proxy.log"),
            user,
        )
    }

    #[test]
    fn a_dead_proxy_is_relaunched_without_a_new_logon() {
        let xml = sample(None);
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<TimeTrigger>"));
        assert!(xml.contains("<Interval>PT1M</Interval>"));
        assert!(
            !xml.contains("<Duration>"),
            "a repetition with a duration stops relaunching when it ends"
        );
    }

    #[test]
    fn a_live_proxy_is_never_doubled_stopped_or_held_back() {
        let xml = sample(None);
        assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
    }

    #[test]
    fn the_command_line_is_escaped_into_valid_xml() {
        let xml = sample(None);
        assert!(xml.contains(
            r"/C &quot;set RTK_PROXY_CONFIG=C:\Users\a&amp;b\AppData\Roaming\rtk-proxy\rtk-proxy.toml &amp;&amp; &quot;C:\Users\a&amp;b\.local\bin\rtk-proxy.exe&quot; start &gt;&gt; &quot;C:\Users\a&amp;b\AppData\Roaming\rtk-proxy\rtk-proxy.log&quot; 2&gt;&amp;1&quot;"
        ));
        let body = xml.split("<Arguments>").nth(1).expect("arguments");
        let body = body.split("</Arguments>").next().expect("closing tag");
        assert!(!body.contains('<') && !body.contains('>') && !body.contains('"'));
        assert!(
            body.replace("&amp;", "")
                .replace("&quot;", "")
                .replace("&gt;", "")
                .find('&')
                .is_none()
        );
    }

    #[test]
    fn the_task_is_scoped_to_the_installing_user_when_known() {
        let xml = sample(Some(r"DESK\me"));
        assert_eq!(xml.matches(r"<UserId>DESK\me</UserId>").count(), 2);
        assert!(!sample(None).contains("<UserId>"));
    }

    #[test]
    fn two_accounts_on_one_machine_get_two_task_names() {
        assert_eq!(task_name(Some(r"DESK\me")), "rtk-proxy-DESK_me");
        assert_ne!(task_name(Some(r"DESK\me")), task_name(Some(r"DESK\you")));
        assert_ne!(task_name(Some(r"DESK\me")), task_name(Some(r"CORP\me")));
        assert_eq!(task_name(Some("a b/c:d*e")), "rtk-proxy-a_b_c_d_e");
    }

    #[test]
    fn an_unknown_user_keeps_the_legacy_task_name() {
        assert_eq!(task_name(None), LEGACY_TASK_NAME);
        assert_eq!(task_name(Some("")), LEGACY_TASK_NAME);
    }

    fn me() -> Account {
        Account {
            sid: Some("S-1-12-1-111-222".into()),
            domain: Some("AzureAD".into()),
            name: "Patrick".into(),
            computer: Some("DESK".into()),
        }
    }

    fn local_me() -> Account {
        Account {
            sid: Some("S-1-5-21-9-1001".into()),
            domain: Some("DESK".into()),
            name: "rtktest".into(),
            computer: Some("DESK".into()),
        }
    }

    /// Shaped like `schtasks /Query /XML` piped on Windows 11, with a logon
    /// trigger naming another user before the principal.
    fn queried(principal: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\r\n<Task>\r\r\n  <Triggers>\r\r\n    \
             <LogonTrigger>\r\r\n      <UserId>OTHER\\someone</UserId>\r\r\n    </LogonTrigger>\r\r\n  \
             </Triggers>\r\r\n  <Principals>\r\r\n    <Principal id=\"Author\">\r\r\n      \
             <UserId>{principal}</UserId>\r\r\n      <LogonType>InteractiveToken</LogonType>\r\r\n    \
             </Principal>\r\r\n  </Principals>\r\r\n</Task>\r\r\n"
        )
    }

    #[test]
    fn the_principal_is_read_not_the_logon_trigger_user() {
        assert_eq!(
            principal_user_id(&queried("S-1-12-1-111-222")).as_deref(),
            Some("S-1-12-1-111-222")
        );
        assert_eq!(
            principal_user_id(&queried("A&amp;B\\me")).as_deref(),
            Some("A&B\\me")
        );
        assert_eq!(principal_user_id("<Task><Principals/></Task>"), None);
        assert_eq!(principal_user_id(""), None);
    }

    #[test]
    fn our_own_task_is_recognised_in_every_form_windows_stores() {
        assert!(owned_by("S-1-12-1-111-222", &me()));
        assert!(owned_by("s-1-12-1-111-222", &me()));
        assert!(owned_by(r"AzureAD\Patrick", &me()));
        assert!(owned_by(r"azuread\patrick", &me()));
        assert!(owned_by("rtktest", &local_me()));
        assert!(owned_by(r"DESK\rtktest", &local_me()));
    }

    #[test]
    fn another_accounts_task_is_never_ours() {
        // What an elevated terminal sees: the task of local account `rtktest`.
        assert!(!owned_by("rtktest", &me()));
        assert!(!owned_by("S-1-5-21-9-1001", &me()));
        assert!(!owned_by("S-1-5-18", &me()));
        assert!(!owned_by(r"AzureAD\someone", &me()));
        // Same name, other domain: a different account.
        assert!(!owned_by(r"CORP\Patrick", &me()));
        // A bare name is a local account, not the Azure AD one with that name.
        assert!(!owned_by("Patrick", &me()));
    }

    #[test]
    fn without_a_known_identity_nothing_is_claimed() {
        let unknown = Account {
            sid: None,
            domain: None,
            name: "rtktest".into(),
            computer: None,
        };
        assert!(!owned_by("S-1-5-21-9-1001", &unknown));
        assert!(!owned_by(r"DESK\rtktest", &unknown));
        assert!(!owned_by("rtktest", &unknown));
        assert!(!owned_by("", &me()));
    }

    #[test]
    fn the_sid_is_read_from_whoami() {
        assert_eq!(
            sid_from_whoami("\"azuread\\patrick\",\"S-1-12-1-111-222\"\r\n").as_deref(),
            Some("S-1-12-1-111-222")
        );
        assert_eq!(sid_from_whoami(""), None);
    }

    #[test]
    fn schtasks_output_is_decoded_from_a_pipe_or_utf16() {
        assert_eq!(decode_output(b"<UserId>x</UserId>"), "<UserId>x</UserId>");
        assert_eq!(decode_output(&encode("<a>é</a>")), "<a>é</a>");
    }

    #[test]
    fn the_file_is_utf16_little_endian_with_a_byte_order_mark() {
        let bytes = encode("<a/>");
        assert_eq!(&bytes[..4], &[0xFF, 0xFE, b'<', 0]);
        assert_eq!(bytes.len(), 2 + 4 * 2);
    }
}
