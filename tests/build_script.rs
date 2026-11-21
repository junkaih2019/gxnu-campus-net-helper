use std::process::{Command, Output};

fn powershell(script: &str) -> Output {
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "RemoteSigned",
            "-Command",
            script,
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Windows PowerShell")
}

#[test]
fn forwards_cargo_and_program_arguments_without_evaluating_them_again() {
    let output = powershell(
        r#"
        function cargo { ConvertTo-Json -InputObject @($args) -Compress; $global:LASTEXITCODE = 0 }
        .\build.ps1 test --locked --offline -- --list
        $prefix = @('test', '--release')
        $tailArgs = @('--exact', 'two words')
        & .\build.ps1 @prefix -- @tailArgs
        $argv = @('run', '--', '--once')
        .\build.ps1 @argv
        .\build.ps1 run '--' 'literal; $value'
        .\build.ps1 run -- -- --
        .\build.ps1 test `
            -- --list | Write-Output
        exit $LASTEXITCODE
    "#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let args: Vec<Vec<String>> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        args,
        [
            vec!["test", "--locked", "--offline", "--", "--list"],
            vec!["test", "--release", "--", "--exact", "two words"],
            vec!["run", "--", "--once"],
            vec!["run", "--", "literal; $value"],
            vec!["run", "--", "--", "--"],
            vec!["test", "--", "--list"],
        ]
    );
}

#[test]
fn verify_stops_at_the_first_failing_cargo_step_and_preserves_its_exit_code() {
    let output = powershell(
        r#"
        function cargo {
            Write-Output "step=$($args[0])"
            $global:LASTEXITCODE = if ($args[0] -eq 'clippy') { 23 } else { 0 }
        }
        .\build.ps1 verify
        exit $LASTEXITCODE
    "#,
    );
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let steps: Vec<_> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("step="))
        .collect();
    assert_eq!(steps, ["fmt", "clippy"]);
}
