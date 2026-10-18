//! Diagnose für den Bildschirmschoner-Abgleich auf diesem Rechner:
//! `screensaver status`      – läuft er gerade?
//! `screensaver watch <s>`   – meldet <s> Sekunden lang Beginn und Ende
//! `screensaver start|stop`  – wie ein Client, wenn der des Servers beginnt bzw. endet
//! `screensaver lock`        – sperrt diesen Rechner wie „Clients mitsperren“

use std::time::{Duration, Instant};

use qross_platform::screensaver;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["status"] => println!("läuft: {}", screensaver::running()),
        ["watch", secs] => watch(Duration::from_secs(secs.parse().expect("Sekunden"))),
        ["start"] => println!("Start: {}", screensaver::start(false)),
        ["stop"] => println!("Ende: {}", screensaver::stop()),
        ["lock"] => println!("Sperren: {}", screensaver::start(true)),
        _ => {
            eprintln!("Aufruf: screensaver status | watch <Sekunden> | start | stop | lock");
            std::process::exit(2);
        }
    }
}

fn watch(limit: Duration) {
    let started = Instant::now();
    let mut last = screensaver::running();
    println!("{:>6.1} s  läuft: {last}", 0.0);
    while started.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(100));
        let now = screensaver::running();
        if now != last {
            println!("{:>6.1} s  läuft: {now}", started.elapsed().as_secs_f32());
            last = now;
        }
    }
}
