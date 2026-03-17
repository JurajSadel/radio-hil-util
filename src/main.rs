use anyhow::{Result, anyhow};
use clap::Parser;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
#[derive(Parser)]
struct Args {
    #[arg(short = 'b', long)]
    binary: PathBuf,

    #[arg(long, default_value = "wifi_ap")]
    ap_test: String,

    #[arg(long, default_value = "wifi_dhcp")]
    sta_test: String,

    #[arg(long)]
    probes: Option<String>,

    #[arg(long, default_value = "120")]
    timeout: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Binary: {}", args.binary.display());

    if !args.binary.exists() {
        return Err(anyhow!("Binary not found: {}", args.binary.display()));
    }

    let probes_str = if let Some(p) = args.probes {
        p
    } else {
        detect_probes()?
    };

    let probes: Vec<&str> = probes_str.split(',').collect();

    if probes.len() != 2 {
        return Err(anyhow!("Expected 2 probes, got {}", probes.len()));
    }

    let ap_probe = probes[0].trim();
    let sta_probe = probes[1].trim();

    println!("AP Test: {} (probe: {})", args.ap_test, ap_probe);
    println!("STA Test: {} (probe: {})", args.sta_test, sta_probe);

    println!("\n Resetting devices...");
    reset_probe(ap_probe)?;
    reset_probe(sta_probe)?;
    println!("✓ Devices reset\n");

    println!("Running tests...");
    let timeout = Duration::from_secs(args.timeout);
    let binary_str = args.binary.to_string_lossy().to_string();
    let ap_test_name = args.ap_test.clone();
    let sta_test_name = args.sta_test.clone();
    let ap_probe_str = ap_probe.to_string();
    let sta_probe_str = sta_probe.to_string();

    let binary_str_cloned = binary_str.clone();

    let ap_should_stop = Arc::new(Mutex::new(false));
    let ap_should_stop_clone = Arc::clone(&ap_should_stop);

    let ap_thread = std::thread::spawn(move || {
        run_test_with_rtt(
            "AP",
            &binary_str_cloned,
            &ap_test_name,
            &ap_probe_str,
            timeout,
            Some(ap_should_stop_clone),
        )
    });

    println!("[STA] Waiting 2000 ms for AP to start...");
    thread::sleep(Duration::from_millis(1000));
    println!("[STA] Starting now...");

    let sta_thread = std::thread::spawn(move || {
        run_test_with_rtt(
            "STA",
            &binary_str,
            &sta_test_name,
            &sta_probe_str,
            timeout,
            None,
        )
    });

    std::thread::sleep(Duration::from_secs(3));

    let sta_result = sta_thread.join().unwrap_or(Ok(false)).unwrap_or(false);

    if sta_result {
        println!("[MAIN] STA passed! Signaling AP to stop...");
        {
            let mut should_stop = ap_should_stop.lock().unwrap();
            *should_stop = true;
        }
        thread::sleep(Duration::from_millis(500));
    }

    let ap_result = ap_thread.join().unwrap_or(Ok(false)).unwrap_or(false);

    println!("AP Test:  {}", if ap_result { "PASSED" } else { "FAILED" });
    println!("STA Test: {}", if sta_result { "PASSED" } else { "FAILED" });

    if ap_result && sta_result {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn detect_probes() -> Result<String> {
    let output = Command::new("probe-rs").args(&["list"]).output()?;

    if !output.status.success() {
        return Err(anyhow!("Failed to run probe-rs list"));
    }

    let list_output = String::from_utf8_lossy(&output.stdout);
    println!("{}", list_output);

    let mut probes = Vec::new();
    for line in list_output.lines() {
        if line.contains("ESP") {
            if let Some(start) = line.find("--") {
                let after_dash = &line[start + 2..].trim();
                if let Some(end) = after_dash.find(' ') {
                    let serial = &after_dash[..end];
                    probes.push(serial.to_string());
                }
            }
        }
    }

    if probes.len() < 2 {
        return Err(anyhow!("Expected 2 probes, found {}", probes.len()));
    }

    println!("Detected probes: {}, {}\n", probes[0], probes[1]);
    Ok(format!("{},{}", probes[0], probes[1]))
}

fn run_test_with_rtt(
    name: &str,
    binary: &str,
    test_name: &str,
    probe: &str,
    timeout: Duration,
    should_stop: Option<Arc<Mutex<bool>>>,
) -> Result<bool> {
    println!("[{}] Running: {}", name, test_name);

    let mut child = Command::new("probe-rs")
        .args(&["run", "--probe", probe, binary, test_name])
        .env("DEFMT_LOG", "info")
        .spawn()
        .map_err(|e| anyhow!("[{}] Failed to spawn: {}", name, e))?;

    println!("[{}] Waiting for test to complete...", name);

    let start = Instant::now();
    loop {
        // Check if we should stop (for AP when STA passes)
        if let Some(ref stop_flag) = should_stop {
            if let Ok(should_stop_val) = stop_flag.lock() {
                if *should_stop_val {
                    println!("[{}] Stopping due to STA completion", name);
                    let _ = child.kill();
                    return Ok(true);
                }
            }
        }

        if start.elapsed() > timeout {
            println!("[{}] TIMEOUT", name);
            let _ = child.kill();
            return Ok(false);
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    println!("[{}] PASSED (process exited successfully)", name);
                    return Ok(true);
                } else {
                    println!(
                        "[{}] FAILED (exit code: {})",
                        name,
                        status.code().unwrap_or(-1)
                    );
                    return Ok(false);
                }
            }
            Ok(None) => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(anyhow!("[{}] Error: {}", name, e));
            }
        }
    }
}

fn reset_probe(probe: &str) -> Result<()> {
    println!("Resetting probe: {}", probe);

    Command::new("probe-rs")
        .args(&["reset", "--probe", probe])
        .output()?;

    thread::sleep(Duration::from_millis(500));

    Ok(())
}
