//! Synthetic executable for bounded diagnostic probes (#1557); no external APIs.
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

fn main() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    if std::env::args().any(|arg| arg == "--hold") {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    let state = PathBuf::from(format!("{}.state", exe.display()));
    fs::write(state.with_extension("pid"), std::process::id().to_string())?;
    let mode = fs::read_to_string(&state)?;
    match mode.trim() {
        "stdout" => println!("tokmd 1.12.0"),
        "stderr" => eprintln!("tokmd 1.12.0"),
        "nonzero" => {
            println!("tokmd 1.12.0");
            eprintln!("SECRET_SENTINEL\x1b[31m::error::injected");
            std::process::exit(7);
        }
        "blank" => {}
        "malformed" => println!("SECRET_SENTINEL\x1b[31m::error::injected"),
        "oversized" | "oversized-stdout" | "oversized-stderr" => {
            println!("tokmd 1.12.0");
            let bytes = [b'x'; 1024];
            for _ in 0..8192 {
                if mode.trim() != "oversized-stderr" {
                    io::stdout().write_all(&bytes)?;
                }
                if mode.trim() != "oversized-stdout" {
                    io::stderr().write_all(&bytes)?;
                }
            }
        }
        "sleep" => loop {
            thread::sleep(Duration::from_secs(1));
        },
        "held" => {
            // Establish a writable ownership receipt before starting a fixture
            // descendant. A publication failure still kills/reaps that child.
            let mut receipt = fs::File::create(state.with_extension("descendant"))?;
            let mut child = Command::new(&exe)
                .arg("--hold")
                .stdin(Stdio::null())
                .spawn()?;
            if let Err(error) = write!(receipt, "{}", child.id()) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            println!("tokmd 1.12.0");
        }
        version if version.starts_with("version:") => println!("{}", &version[8..]),
        _ => std::process::exit(8),
    }
    Ok(())
}
