//! Every process Marsad starts (daemon, hub, desktop shell) goes through wasit-process, so no
//! console window ever opens on Windows.

use std::path::Path;

#[test]
fn no_process_is_created_outside_wasit_process() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = wasit_process::scan::raw_spawns(&root.join("crates"), &[]);
    found.extend(wasit_process::scan::raw_spawns(
        &root.join("apps/desktop/src-tauri/src"),
        &[],
    ));
    assert!(
        found.is_empty(),
        "create processes with wasit_process::{{command, std_command, detached_command}}, not Command::new:\n{}",
        found.join("\n")
    );
}
