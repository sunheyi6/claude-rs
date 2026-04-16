use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn spawn_cli() -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_claude-rs"))
        .arg("--api-key")
        .arg("test-key")
        .env("CLAUDE_RS_TEST_MODE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn claude-rs")
}

fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            return status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            panic!("process timeout after {:?}", timeout);
        }
        thread::sleep(Duration::from_millis(30));
    }
}

fn wait_with_output_timeout(
    child: std::process::Child,
    timeout: Duration,
) -> std::process::Output {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let out = child.wait_with_output();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => panic!("wait_with_output failed: {e}"),
        Err(_) => panic!("process timeout after {:?}", timeout),
    }
}

#[test]
fn startup_waits_for_input_and_can_exit_cleanly() {
    let mut child = spawn_cli();

    // Ensure process keeps running while waiting for user input.
    thread::sleep(Duration::from_millis(250));
    assert!(child.try_wait().expect("try_wait failed").is_none());

    {
        let stdin = child.stdin.as_mut().expect("stdin missing");
        stdin.write_all(b"/quit\n").expect("write stdin failed");
        stdin.flush().expect("flush stdin failed");
    }

    let status = wait_with_timeout(&mut child, Duration::from_secs(5));
    assert!(status.success(), "exit status: {status:?}");
}

#[test]
fn input_is_processed_and_echoed_in_test_mode() {
    let mut child = spawn_cli();
    {
        let stdin = child.stdin.as_mut().expect("stdin missing");
        stdin
            .write_all("你好，测试输入\n/quit\n".as_bytes())
            .expect("write stdin failed");
        stdin.flush().expect("flush stdin failed");
    }

    let output = wait_with_output_timeout(child, Duration::from_secs(8));
    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("TEST_ECHO: 你好，测试输入"),
        "stdout was:\n{}",
        stdout
    );
}

#[test]
fn empty_line_and_unknown_command_do_not_panic() {
    let mut child = spawn_cli();
    {
        let stdin = child.stdin.as_mut().expect("stdin missing");
        stdin
            .write_all(b"\n/unknown-command\n/quit\n")
            .expect("write stdin failed");
        stdin.flush().expect("flush stdin failed");
    }

    let output = wait_with_output_timeout(child, Duration::from_secs(8));
    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("错误：未知命令（test mode）"),
        "stdout was:\n{}",
        stdout
    );
    assert!(
        !stdout.contains("panicked") && !stderr.contains("panicked"),
        "stdout:\n{}\n\nstderr:\n{}",
        stdout,
        stderr
    );
}
