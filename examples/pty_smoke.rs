use anyhow::Result;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows: 12, cols: 60, pixel_width: 0, pixel_height: 0 })?;

    let mut cmd = CommandBuilder::new("bash");
    cmd.arg("-c");
    cmd.arg(r#"printf '\033[1;31mRED\033[0m bold\r\n'; printf '\033[32mgreen\033[0m\r\n'; printf 'line three\r\n'"#);
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let raw: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

    let raw_t = raw.clone();
    let t = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => raw_t.lock().unwrap().extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
    });

    let _ = child.wait();
    thread::sleep(Duration::from_millis(50));
    drop(pair.master);
    let _ = t.join();

    let buf = raw.lock().unwrap();
    println!("--- raw PTY bytes ({} bytes) ---", buf.len());
    println!("{}", String::from_utf8_lossy(&buf));
    println!("--- end ---");
    Ok(())
}
