mod common;

#[cfg(unix)]
mod tests {
    use super::common;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn explicit_lambda_formats_preserve_native_streams_arguments_and_status() {
        let temp = common::tempdir();
        let aws = temp.path().join("aws");
        let argv = temp.path().join("argv");
        fs::write(
            &aws,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$AWS_ARGV\"\nprintf '%s' \"$AWS_STDOUT\"\nprintf '%s' \"$AWS_STDERR\" >&2\nexit \"$AWS_EXIT\"\n",
        )
        .unwrap();
        fs::set_permissions(&aws, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        for operation in [
            "get-function",
            "get-function-configuration",
            "list-functions",
        ] {
            for (format, raw) in [
                (
                    "json",
                    "{\n  \"FunctionName\": \"sample\", \"Code\": {\"ImageUri\": \"example/image:v1\"}, \"Environment\": {\"Variables\": {\"EXAMPLE\": \"value\"}}\n}\n",
                ),
                (
                    "yaml",
                    "FunctionName: sample\nCode:\n  ImageUri: example/image:v1\n",
                ),
                ("yaml-stream", "- FunctionName: sample\n  State: Active\n"),
                ("text", "sample\tActive\n"),
            ] {
                for attached in [false, true] {
                    let mut args = vec![
                        "lambda".to_owned(),
                        operation.to_owned(),
                        "--profile".to_owned(),
                        "profile with spaces".to_owned(),
                    ];
                    if attached {
                        args.push(format!("--output={format}"));
                    } else {
                        args.extend(["--output".to_owned(), format.to_owned()]);
                    }
                    for code in [0, 23] {
                        let output = common::rtk_command()
                            .arg("aws")
                            .args(&args)
                            .env("PATH", &path)
                            .env("AWS_ARGV", &argv)
                            .env("AWS_STDOUT", raw)
                            .env("AWS_STDERR", "native diagnostic\n")
                            .env("AWS_EXIT", code.to_string())
                            .output()
                            .unwrap();
                        assert_eq!(output.status.code(), Some(code), "{args:?}");
                        assert_eq!(fs::read_to_string(&argv).unwrap(), args.join("\n") + "\n");
                        assert_eq!(output.stdout, raw.as_bytes(), "{args:?}");
                        assert_eq!(output.stderr, b"native diagnostic\n", "{args:?}");
                    }
                }
            }
        }
    }
}
