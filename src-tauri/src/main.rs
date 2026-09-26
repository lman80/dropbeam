// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  #[cfg(not(any(target_os = "ios", target_os = "android")))]
  {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--server") {
      app_lib::run_headless(&args);
      return;
    }
  }
  app_lib::run();
}
