use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let Some(rc_exe) = find_rc_exe() else {
        println!("cargo:warning=rc.exe not found; executable will not embed app.ico");
        return;
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let res_path = out_dir.join("app.res");
    let status = Command::new(rc_exe)
        .args(["/nologo", "/fo"])
        .arg(&res_path)
        .arg("res/app.rc")
        .status()
        .expect("failed to invoke rc.exe");

    if !status.success() {
        panic!("rc.exe failed with status: {status}");
    }

    println!("cargo:rustc-link-arg-bins={}", res_path.display());
}

fn find_rc_exe() -> Option<PathBuf> {
    if let Ok(path_var) = env::var("PATH") {
        for dir in env::split_paths(&path_var) {
            let cand = dir.join("rc.exe");
            if cand.is_file() {
                return Some(cand);
            }
        }
    }

    let kits_root = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let entries = std::fs::read_dir(kits_root).ok()?;
    let mut versions: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    versions.reverse();

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    for ver in versions {
        let cand = ver.join(arch).join("rc.exe");
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}
