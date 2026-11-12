use std::path::{Path, PathBuf};

const LIBS: [&str; 2] = ["libc++.a", "libc++abi.a"];

const SYSTEM_DIRS: [&str; 7] = [
    "/usr/lib",
    "/usr/lib64",
    "/usr/local/lib",
    "/usr/lib/x86_64-pc-linux-gnu",
    "/usr/lib/x86_64-linux-gnu",
    "/usr/lib/aarch64-linux-gnu",
    "/usr/lib/aarch64-unknown-linux-gnu",
];

fn has_libs(dir: &Path) -> bool {
    LIBS.iter().all(|l| dir.join(l).is_file())
}

fn llvm_dirs() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/usr/lib") else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("llvm-"))
        .map(|e| e.path().join("lib"))
        .collect();
    dirs.sort();
    dirs.reverse();
    dirs
}

fn static_only_dir(found: &Path) -> PathBuf {
    let out =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("libcxx");
    std::fs::create_dir_all(&out)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", out.display()));
    for lib in LIBS {
        let link = out.join(lib);
        match std::fs::remove_file(&link) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("cannot replace {}: {e}", link.display()),
        }
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(found.join(lib), &link);
        #[cfg(not(unix))]
        let linked = std::fs::copy(found.join(lib), &link).map(|_| ());
        linked.unwrap_or_else(|e| panic!("cannot link {} into {}: {e}", lib, out.display()));
    }
    out
}

fn main() {
    println!("cargo:rerun-if-env-changed=LIBCXX_DIR");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    if let Some(dir) = std::env::var_os("LIBCXX_DIR").filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        if !has_libs(&dir) {
            panic!(
                "LIBCXX_DIR={} must contain {} and {}",
                dir.display(),
                LIBS[0],
                LIBS[1]
            );
        }
        println!("cargo:rustc-link-search=native={}", dir.display());
        return;
    }
    let candidates: Vec<PathBuf> = SYSTEM_DIRS
        .iter()
        .map(PathBuf::from)
        .chain(llvm_dirs())
        .chain(
            std::env::var_os("CARGO_MANIFEST_DIR").map(|d| PathBuf::from(d).join(".work/libcxx")),
        )
        .collect();
    println!("cargo:rerun-if-changed=.work/libcxx");
    match candidates.iter().find(|d| has_libs(d)) {
        Some(found) => {
            println!(
                "cargo:rustc-link-search=native={}",
                static_only_dir(found).display()
            );
        }
        None => panic!(
            "TDLib is linked statically and needs the static LLVM C++ runtime ({} and {}), \
             which was not found in {}.\n\
             Install it: Arch `pacman -S libc++ libc++abi`, Debian/Ubuntu `apt install libc++-dev libc++abi-dev`, \
             Fedora `dnf install libcxx-static libcxxabi-static`.\n\
             Or point LIBCXX_DIR at a directory that holds both files.",
            LIBS[0],
            LIBS[1],
            candidates
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
