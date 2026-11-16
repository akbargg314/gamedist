//! Headless-only build of the launcher. The graphical app in `apps/launcher`
//! links the same engine and forwards `--headless` invocations here.

fn main() -> std::process::ExitCode {
    let informational = ["--headless", "--help", "-h", "--version", "-V"];
    if !std::env::args().any(|a| informational.contains(&a.as_str())) {
        eprintln!(
            "this binary is the headless launcher; run it with --headless (see --help), \
             or build the graphical app in apps/launcher"
        );
        return std::process::ExitCode::from(2);
    }
    gamedist_launcher::headless::main_from_args()
}
