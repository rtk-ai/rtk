use std::path::Path;

const RELAUNCH_INTERVAL: &str = "PT1M";

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
      <StartBoundary>2020-01-01T00:00:00</StartBoundary>
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
    fn the_file_is_utf16_little_endian_with_a_byte_order_mark() {
        let bytes = encode("<a/>");
        assert_eq!(&bytes[..4], &[0xFF, 0xFE, b'<', 0]);
        assert_eq!(bytes.len(), 2 + 4 * 2);
    }
}
