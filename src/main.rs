use clap::Parser;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{channel, Sender},
    thread,
    time::{Duration, Instant},
};

#[derive(Parser, Debug)]
#[command(author, version, about = "Auto-reload Flutter on file changes")]
struct Args {
    /// Path to Flutter project
    #[arg(default_value = ".")]
    project_path: PathBuf,

    /// Flutter run arguments (e.g. --device-id=xxx)
    #[arg(last = true)]
    flutter_args: Vec<String>,

    /// Debounce duration in milliseconds
    #[arg(long, default_value = "1000")]
    debounce: u64,

    /// Device ID to run on
    #[arg(short = 'd', long)]
    device_id: Option<String>,

    /// Flutter flavor to use
    #[arg(short, long)]
    flavor: Option<String>,

    /// Release mode
    #[arg(short, long)]
    release: bool,

    /// Profile mode
    #[arg(long)]
    profile: bool,
}

enum FlutterCommand {
    Reload,
    KeyInput(u8),
}

struct FlutterRunner {
    process: Child,
    last_reload: Instant,
    debounce_duration: Duration,
}

impl FlutterRunner {
    fn new(args: &Args) -> std::io::Result<Self> {
        let mut command = Command::new("flutter");
        command.arg("run");

        if let Some(device_id) = &args.device_id {
            command.arg("--device-id").arg(device_id);
        }

        if let Some(flavor) = &args.flavor {
            command.arg("--flavor").arg(flavor);
        }

        if args.release {
            command.arg("--release");
        } else if args.profile {
            command.arg("--profile");
        }

        command.args(&args.flutter_args);

        let process = command
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .current_dir(&args.project_path)
            .spawn()?;

        Ok(Self {
            process,
            last_reload: Instant::now(),
            debounce_duration: Duration::from_millis(args.debounce),
        })
    }

    fn handle_command(&mut self, cmd: FlutterCommand) -> std::io::Result<()> {
        match cmd {
            FlutterCommand::Reload => {
                let now = Instant::now();
                if now.duration_since(self.last_reload) >= self.debounce_duration {
                    println!("\n🔄 Change detected, triggering hot reload...");
                    if let Some(stdin) = self.process.stdin.as_mut() {
                        stdin.write_all(b"r\n")?;
                        stdin.flush()?;
                    }
                    self.last_reload = now;
                }
            }
            FlutterCommand::KeyInput(key) => {
                if let Some(stdin) = self.process.stdin.as_mut() {
                    stdin.write_all(&[key])?;
                    stdin.flush()?;
                }
            }
        }
        Ok(())
    }
}

impl Drop for FlutterRunner {
    fn drop(&mut self) {
        let _ = self.process.kill();
    }
}

fn handle_keyboard_input(tx: Sender<FlutterCommand>) -> std::io::Result<()> {
    let mut stdin = io::stdin();
    let mut buffer = [0; 1];

    loop {
        if stdin.read_exact(&mut buffer).is_ok() {
            tx.send(FlutterCommand::KeyInput(buffer[0])).ok();
        }
    }
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let pubspec_path = args.project_path.join("pubspec.yaml");
    if !pubspec_path.exists() {
        eprintln!("Error: Not a valid Flutter project directory");
        std::process::exit(1);
    }

    println!("🚀 Starting Flutter run with auto-reload...");
    println!("📁 Project path: {}", args.project_path.display());
    if let Some(device_id) = &args.device_id {
        println!("📱 Device ID: {}", device_id);
    }
    if let Some(flavor) = &args.flavor {
        println!("🔧 Flavor: {}", flavor);
    }
    if !args.flutter_args.is_empty() {
        println!("⚙️  Additional args: {}", args.flutter_args.join(" "));
    }

    let mut flutter = FlutterRunner::new(&args)?;

    // Channel for file watcher events
    let (file_tx, file_rx) = channel();
    let mut watcher = RecommendedWatcher::new(
        file_tx,
        Config::default().with_poll_interval(Duration::from_secs(1)),
    )
    .unwrap();

    // Channel for keyboard input
    let (input_tx, input_rx) = channel();

    // Watch the project directory
    watcher
        .watch(&args.project_path, RecursiveMode::Recursive)
        .unwrap();

    println!("✨ Auto-reload is now active. Watching for changes...");
    println!("💡 You can use all Flutter commands (r = reload, R = restart, h = help)");

    // Spawn thread for keyboard input
    thread::spawn(move || {
        handle_keyboard_input(input_tx).ok();
    });

    // Main event loop
    loop {
        // Check for file changes
        if let Ok(event) = file_rx.try_recv() {
            if let Ok(event) = event {
                if let Some(path) = event.paths.first() {
                    if let Some(ext) = path.extension() {
                        if ext == "dart" {
                            flutter.handle_command(FlutterCommand::Reload)?;
                        }
                    }
                }
            }
        }

        // Check for keyboard input
        if let Ok(cmd) = input_rx.try_recv() {
            flutter.handle_command(cmd)?;
        }

        // Small sleep to prevent busy waiting
        thread::sleep(Duration::from_millis(10));
    }
}
