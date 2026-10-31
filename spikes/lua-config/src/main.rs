//! tileide-lua-spike check <file> [--dump]   validate (the future `tileide --check-config`)
//! tileide-lua-spike watch <file>            hot-reload on every save, keep last good config
//! tileide-lua-spike bench <file> [runs]     cold load timings

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use tileide_lua_spike::{FLAVOUR, Limits, Loaded, load};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(command), Some(file)) = (args.first(), args.get(1)) else {
        eprintln!("usage: tileide-lua-spike <check|watch|bench> <file> [--dump | runs]");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(file);
    match command.as_str() {
        "check" => check(&path, args.iter().any(|a| a == "--dump")),
        "watch" => watch(&path),
        "bench" => bench(&path, args.get(2).and_then(|n| n.parse().ok()).unwrap_or(200).max(1)),
        _ => {
            eprintln!("unknown command {command}");
            ExitCode::from(2)
        }
    }
}

fn report(loaded: &Loaded) {
    for warning in &loaded.warnings {
        eprintln!("{warning}");
    }
    let c = &loaded.config;
    println!(
        "ok ({FLAVOUR}, {:.2} ms): theme={} window_radius={} mod={} binds={} workspaces={}",
        loaded.elapsed.as_secs_f64() * 1000.0,
        c.appearance.theme,
        c.appearance.window_radius,
        c.keys.modifier,
        c.binds.len(),
        c.workspaces.len()
    );
}

fn check(path: &Path, dump: bool) -> ExitCode {
    match load(path, Limits::default()) {
        Ok(loaded) => {
            report(&loaded);
            if dump {
                println!("{:#?}", loaded.config);
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn watch(path: &Path) -> ExitCode {
    let mut current = match load(path, Limits::default()) {
        Ok(loaded) => {
            report(&loaded);
            loaded.config
        }
        Err(message) => {
            eprintln!("error: {message}\n(starting with defaults)");
            Default::default()
        }
    };

    let (tx, rx) = mpsc::channel();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("cannot watch: {e}");
            return ExitCode::FAILURE;
        }
    };
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
        eprintln!("cannot watch {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    println!("watching {} (Ctrl-C to quit)", dir.display());

    while let Ok(event) = rx.recv() {
        let touches_lua = event
            .map(|e| e.kind.is_modify() || e.kind.is_create())
            .unwrap_or(false);
        if !touches_lua {
            continue;
        }
        // Editors save in several steps; let the burst settle before reloading.
        let saved = Instant::now();
        while rx.recv_timeout(Duration::from_millis(30)).is_ok() {}
        match load(path, Limits::default()) {
            Ok(loaded) if loaded.config == current => {
                println!("reloaded: no change ({:.2} ms after save)", saved.elapsed().as_secs_f64() * 1000.0)
            }
            Ok(loaded) => {
                print!("reloaded {:.2} ms after save (incl. 30 ms debounce) → ", saved.elapsed().as_secs_f64() * 1000.0);
                report(&loaded);
                current = loaded.config;
            }
            Err(message) => eprintln!("error: {message}\nkeeping last good config (window_radius={})", current.appearance.window_radius),
        }
    }
    ExitCode::SUCCESS
}

fn bench(path: &Path, runs: usize) -> ExitCode {
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        match load(path, Limits::default()) {
            Ok(loaded) => times.push(loaded.elapsed),
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::FAILURE;
            }
        }
    }
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "{FLAVOUR}: {runs} cold loads (new state + full config): min {:.3} ms, median {:.3} ms, max {:.3} ms",
        ms(times[0]),
        ms(times[runs / 2]),
        ms(times[runs - 1])
    );
    ExitCode::SUCCESS
}
