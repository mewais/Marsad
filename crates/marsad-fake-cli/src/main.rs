//! The fake agent CLI (`wasit-fake-cli`), built here so integration tests find it next to
//! their own binaries.

fn main() {
    std::process::exit(wasit_fake_cli::cli::main_with(
        std::env::args().skip(1).collect(),
    ));
}
