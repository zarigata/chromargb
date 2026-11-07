//! chromargb-cli: `list` (discovery table) and `doctor` (self-checks, no
//! lighting tests).

use chromargb_drivers::hidapi::HidApi;
use chromargb_drivers::{devices_table, discover, open_backend};

fn main() {
    let code = match std::env::args().nth(1).as_deref() {
        Some("list") => list(),
        Some("doctor") => doctor(),
        _ => {
            eprintln!("usage: chromargb-cli <list|doctor>");
            2
        }
    };
    std::process::exit(code);
}

fn list() -> i32 {
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("hidapi init failed: {e}");
            return 1;
        }
    };
    let devices = discover(&api);
    print!("{}", devices_table(&devices));
    if devices.is_empty() {
        println!("no recognized RGB devices found");
    }
    0
}

fn doctor() -> i32 {
    println!("chromargb doctor");
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            println!("  [FAIL] hidapi init: {e}");
            return 1;
        }
    };
    println!("  [PASS] hidapi initialized");

    let devices = discover(&api);
    println!("  [INFO] {} recognized RGB device(s)", devices.len());

    let mut fails = 0;
    let mut warns = 0;
    for d in &devices {
        let name = format!("{} ({})", d.info.name, d.info.id);
        match open_backend(d) {
            Ok(_) => println!("  [PASS] {name}: backend constructed"),
            Err(e) => {
                fails += 1;
                println!("  [FAIL] {name}: backend construct failed: {e}");
            }
        }
        // Open probe only — no lighting commands are sent.
        match api.open_path(&d.path) {
            Ok(_) => println!("  [PASS] {name}: hidapi open ok (no lighting test performed)"),
            Err(e) => {
                warns += 1;
                println!(
                    "  [WARN] {name}: hidapi open failed: {e} — possibly a permissions issue; discovery still works"
                );
            }
        }
    }
    println!("doctor: {fails} fail(s), {warns} warning(s)");
    if fails > 0 {
        1
    } else {
        0
    }
}
