use std::env;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn print_help() {
    println!("Enhanced Windows Clock Hover Hook CLI");
    println!("====================================");
    println!();
    println!("USAGE:");
    println!("    xclock-cli [COMMAND]");
    println!();
    println!("COMMANDS:");
    println!("    start     Start the clock hover hook");
    println!("    stop      Stop the clock hover hook (if running)");
    println!("    status    Check if the hook is running");
    println!("    help      Show this help message");
    println!();
    println!("EXAMPLES:");
    println!("    xclock-cli start    # Start monitoring the clock");
    println!("    xclock-cli stop     # Stop the hook");
    println!("    xclock-cli status   # Check running status");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        print_help();
        return;
    }

    match args[1].as_str() {
        "start" => {
            println!("Starting Windows Clock Hover Hook...");
            
            // Set up Ctrl+C handler
            let running = Arc::new(AtomicBool::new(true));
            let r = running.clone();
            
            ctrlc::set_handler(move || {
                println!("\nShutting down...");
                r.store(false, Ordering::SeqCst);
            }).expect("Error setting Ctrl+C handler");

            // Start the hook
            match xclock::start_monitoring() {
                Ok(()) => {
                    println!("Hook started successfully!");
                    println!("Hover over the system clock to see extended information.");
                    println!("Press Ctrl+C to exit.");
                    
                    // Main message loop
                    while running.load(Ordering::SeqCst) && xclock::is_running() {
                        match xclock::message_loop() {
                            Ok(()) => break,
                            Err(_) => {
                                thread::sleep(Duration::from_millis(10));
                            }
                        }
                    }
                    
                    // Clean shutdown
                    xclock::stop_monitoring();
                    println!("Program terminated.");
                },
                Err(e) => {
                    eprintln!("Failed to start hook: {}", e);
                    process::exit(1);
                }
            }
        },
        
        "stop" => {
            println!("Stopping clock hover hook...");
            xclock::stop_monitoring();
            println!("Hook stopped.");
        },
        
        "status" => {
            if xclock::is_running() {
                println!("Clock hover hook is currently RUNNING");
            } else {
                println!("Clock hover hook is currently STOPPED");
            }
        },
        
        "help" | "--help" | "-h" => {
            print_help();
        },
        
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            eprintln!("Use 'xclock-cli help' for usage information.");
            process::exit(1);
        }
    }
}
