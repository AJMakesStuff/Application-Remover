mod backend;

use backend::{Inventory, Package, RemovalPlan};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, Read};
use std::process::{Command, ExitCode};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    Inventory,
    Details {
        package: Package,
    },
    RemovalPlan {
        package: Package,
        packages: Vec<Package>,
    },
    Remove {
        package: Package,
        packages: Vec<Package>,
        plan: RemovalPlan,
    },
}

fn execute(request: Request) -> Result<Value, String> {
    let result = match request {
        Request::Inventory => serde_json::to_value::<Inventory>(backend::inventory()),
        Request::Details { package } => {
            return backend::package_details(&package).and_then(|details| {
                serde_json::to_value(details).map_err(|error| error.to_string())
            });
        }
        Request::RemovalPlan { package, packages } => {
            return backend::removal_plan(&package, &packages)
                .and_then(|plan| serde_json::to_value(plan).map_err(|error| error.to_string()));
        }
        Request::Remove {
            package,
            packages,
            plan,
        } => {
            return backend::remove(&package, &packages, &plan).and_then(|output| {
                serde_json::to_value(output).map_err(|error| error.to_string())
            });
        }
    };
    result.map_err(|error| error.to_string())
}

fn run_backend() -> ExitCode {
    let mut input = String::new();
    let response = match io::stdin().read_to_string(&mut input) {
        Ok(_) => match serde_json::from_str::<Request>(&input) {
            Ok(request) => match execute(request) {
                Ok(result) => json!({ "ok": true, "result": result }),
                Err(error) => json!({ "ok": false, "error": error }),
            },
            Err(error) => {
                json!({ "ok": false, "error": format!("Invalid backend request: {error}") })
            }
        },
        Err(error) => {
            json!({ "ok": false, "error": format!("Could not read backend request: {error}") })
        }
    };
    println!("{response}");
    ExitCode::SUCCESS
}

fn launch_frontend() -> ExitCode {
    let backend = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Could not locate the application-remover executable: {error}");
            return ExitCode::FAILURE;
        }
    };
    let python = std::env::var_os("APPLICATION_REMOVER_PYTHON").unwrap_or_else(|| "python3".into());
    let status = Command::new(python)
        .args(["-m", "application_remover"])
        .env("APPLICATION_REMOVER_BACKEND", backend)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status();
    match status {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(error) => {
            eprintln!("Could not start the GTK frontend with Python: {error}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--backend") => run_backend(),
        Some("--help") | Some("-h") => {
            println!("Application Remover\n\nRun without options to open the GTK application.");
            ExitCode::SUCCESS
        }
        Some(argument) => {
            eprintln!("Unknown option: {argument}\nRun with --help for usage.");
            ExitCode::from(2)
        }
        None => launch_frontend(),
    }
}
