use std::path::Path;
use std::process::Command;

pub fn open_abs_in_server_at(abs: &Path, server: &Path, line: Option<u32>) -> Result<(), String> {
    let server = server.to_string_lossy();
    let mut open = Command::new("nvim");
    open.args(["--server", &server, "--remote", &abs.to_string_lossy()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    open.status()
        .map_err(|e| format!("Failed to send nvim --remote: {e}"))?;
    let Some(line) = line else {
        return Ok(());
    };
    Command::new("nvim")
        .args([
            "--server",
            &server,
            "--remote-send",
            &format!("<Cmd>normal! {line}Gzz<CR>"),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|e| format!("Failed to send nvim --remote-send: {e}"))
}
