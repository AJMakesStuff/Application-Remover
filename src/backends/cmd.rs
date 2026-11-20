//! Process helpers. Every command is a structured argv; no shell is involved.

use crate::models::{CommandSpec, Error, Result};
use crate::security;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

fn base_command(program: &str, args: &[&str]) -> Result<Command> {
    let (prog, argv): (String, Vec<String>) = if security::in_flatpak_sandbox() {
        let mut v = vec!["--host".to_string(), program.to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        ("flatpak-spawn".into(), v)
    } else {
        let p = security::find_program(program).ok_or(Error::Unavailable("required program"))?;
        (p.to_string_lossy().into_owned(), args.iter().map(|s| s.to_string()).collect())
    };
    let mut c = Command::new(prog);
    c.args(argv).env("LC_ALL", "C").env("LANG", "C").stdin(Stdio::null());
    Ok(c)
}

/// Run a read-only command and return stdout. Non-zero exit is an error.
pub fn run(program: &str, args: &[&str]) -> Result<String> {
    let out = run_allow_failure(program, args)?;
    if out.0 {
        Ok(out.1)
    } else {
        Err(Error::Command {
            program: program.into(),
            message: out.2.trim().lines().last().unwrap_or("non-zero exit").to_string(),
        })
    }
}

/// Run a command and return (success, stdout, stderr).
pub fn run_allow_failure(program: &str, args: &[&str]) -> Result<(bool, String, String)> {
    let output = base_command(program, args)?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| Error::Spawn { program: program.into(), message: e.to_string() })?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

pub fn exists(program: &str) -> bool {
    if security::in_flatpak_sandbox() {
        return run_allow_failure("which", &[program]).map(|r| r.0).unwrap_or(false);
    }
    security::find_program(program).is_some()
}

/// Execute a removal command, streaming combined output line by line.
/// `cancel` is honoured only when the spec is marked cancellable.
pub fn run_streaming(spec: &CommandSpec, cancel: &Arc<AtomicBool>, on_line: &mut dyn FnMut(&str)) -> Result<()> {
    let (program, args) = security::build_argv(spec)?;
    let mut cmd = Command::new(&program);
    cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if !spec.privileged && !security::in_flatpak_sandbox() {
        cmd.env("LC_ALL", "C");
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
    }
    let mut child = cmd.spawn().map_err(|e| Error::Spawn { program: program.clone(), message: e.to_string() })?;

    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(err, tx.clone()));
    }
    drop(tx);

    let mut killed = false;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(150)) {
            Ok(line) => on_line(&line),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if spec.cancellable && !killed && cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            killed = true;
        }
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait().map_err(|e| Error::Spawn { program: program.clone(), message: e.to_string() })?;
    if killed {
        return Err(Error::Cancelled);
    }
    if status.success() {
        return Ok(());
    }
    if security::is_auth_failure(spec, status.code()) {
        return Err(Error::AuthDenied);
    }
    Err(Error::Command {
        program: spec.program.clone(),
        message: format!(
            "exited with status {}",
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
        ),
    })
}

fn spawn_reader<R: Read + Send + 'static>(r: R, tx: mpsc::Sender<String>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // Package managers print progress with carriage returns; split on both.
        let mut reader = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&buf);
                    for part in text.split(['\n', '\r']) {
                        if !part.trim().is_empty() && tx.send(part.trim_end().to_string()).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    })
}
