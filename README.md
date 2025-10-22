1. Install Rust on Ubuntu/Windows.
  - Install Rust using rustup: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    Or on Windows, download and run [rustup-init.exe](https://rustup.rs/)
  - Restart terminal or run:  source $HOME/.cargo/env
  - Verify installation: rustc --version
    
2. Build project.
  - git clone <repository-url>
    cd my-rust-project
  - cargo build --release (Install pkg-config using apt install pkg-config command)
3. Run project with build result or cargo run command with required parameters.
  - cd target/release
  - ./regbot --coldkey="" --hotkey=""
