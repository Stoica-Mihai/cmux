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
    let parser = Arc::new(Mutex::new(vt100::Parser::new(12, 60, 1024)));

    let p = parser.clone();
    let t = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => p.lock().unwrap().process(&buf[..n]),
                Err(_) => break,
            }
        }
    });

    let _ = child.wait();
    thread::sleep(Duration::from_millis(50));
    drop(pair.master);
    let _ = t.join();

    let g = parser.lock().unwrap();
    let screen = g.screen();
    println!("--- vt100 rendered contents ({} rows) ---", screen.size().0);
    println!("{}", screen.contents());
    println!("--- end ---");

    let row0_red = screen.cell(0, 0).map(|c| format!("'{}' fg={:?}", c.contents(), c.fgcolor())).unwrap_or_default();
    println!("cell(0,0) = {}", row0_red);
    Ok(())
}
