use std::{env, fs, path::Path, process::Command};

fn fetch_tpn_subnet_main_hash() -> String {
    let output = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github.v3.sha",
            "https://api.github.com/repos/taofu-labs/tpn-subnet/commits/main",
        ])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let body = String::from_utf8_lossy(&out.stdout);
            let sha = body.trim();
            if sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return sha[..7].to_string();
            }
        }
    }
    "unknown".to_string()
}

fn main() {
    // Always fetch and inject git hash (build always re-runs because of this directive)
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=TPN_SUBNET_GIT_HASH");

    let hash = env::var("TPN_SUBNET_GIT_HASH").unwrap_or_else(|_| fetch_tpn_subnet_main_hash());
    println!("cargo:rustc-env=TPN_SUBNET_GIT_HASH={}", hash);

    // Only run when the embed feature is enabled.
    if env::var_os("CARGO_FEATURE_EMBED_IP2LOCATION").is_none() {
        return;
    }

    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let default_path = Path::new(&crate_dir)
        .join("ip2location_data")
        .join("ip2location.zip");
    let embed_path = env::var("IP2LOCATION_EMBED_ARCHIVE")
        .or_else(|_| env::var("IP2LOCATION_EMBED_BIN"))
        .map(Into::into)
        .unwrap_or(default_path);

    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("ip2location_embedded.rs");

    if embed_path.exists() {
        println!("cargo:rerun-if-changed={}", embed_path.display());
        fs::write(
            &out,
            format!(
                "pub const EMBED_IP2LOCATION_ZIP: &[u8] = include_bytes!(\"{}\");",
                embed_path.display()
            ),
        )
        .expect("write embedded module");
    } else {
        println!(
            "cargo:warning=embed-ip2location enabled but archive not found at {}",
            embed_path.display()
        );
        // Write a stub so include! still succeeds; runtime will simply skip embedding.
        fs::write(&out, "pub const EMBED_IP2LOCATION_ZIP: &[u8] = &[];\n")
            .expect("write stub embedded module");
    }
}
