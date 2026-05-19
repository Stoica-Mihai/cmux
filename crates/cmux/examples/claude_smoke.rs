use anyhow::Result;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize {
        rows: 30,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new("claude");
    cmd.arg("--help");
    cmd.cwd(std::env::current_dir()?);
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let raw: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

    let raw_t = raw.clone();
    let t = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut total = 0usize;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    raw_t.lock().unwrap().extend_from_slice(&buf[..n]);
                }
                Err(_) => break,
            }
        }
        total
    });

    let _ = child.wait();
    thread::sleep(Duration::from_millis(100));
    drop(pair.master);
    let bytes = t.join().unwrap_or(0);

    let buf = raw.lock().unwrap();
    println!("--- {} bytes received ---", bytes);
    println!("{}", String::from_utf8_lossy(&buf));
    println!("--- end ---");
    Ok(())
}
