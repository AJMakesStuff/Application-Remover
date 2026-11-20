//! Developer tool: scan the real system and print what was detected.
//! Read-only: never removes anything. `cargo run --example scan --no-default-features [-- --system] [--preview <source> <id>]`
use lar::models::*;
use lar::services::{removal, scan};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let show_system = args.iter().any(|a| a == "--system");
    let t = std::time::Instant::now();
    let r = scan::scan();
    println!("scan took {:?}; sources: {:?}; errors: {:?}", t.elapsed(), r.sources, r.errors);
    let apps = r.packages.iter().filter(|p| p.kind == Kind::Application).count();
    println!("{} packages total, {} applications", r.packages.len(), apps);
    for p in r.packages.iter().filter(|p| show_system || p.kind == Kind::Application) {
        println!(
            "{:<8} {:<34} {:<28} {:<14} {:>10} {}",
            p.source().label(),
            p.name.chars().take(34).collect::<String>(),
            p.publisher.clone().unwrap_or_default().chars().take(28).collect::<String>(),
            p.version.chars().take(14).collect::<String>(),
            p.size.map(format_size).unwrap_or_default(),
            if p.kind == Kind::System {
                format!("[system: {}]", p.system_reason.clone().unwrap_or_default())
            } else {
                String::new()
            }
        );
    }
    if let Some(i) = args.iter().position(|a| a == "--preview") {
        let id = &args[i + 1];
        let p = r.packages.iter().find(|p| &p.id == id).expect("no such package");
        let prep = removal::prepare(p).expect("prepare");
        println!("{:#?}", prep);
    }
}
