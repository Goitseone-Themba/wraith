//! Self-signed certificate generation so the binary works without openssl.

use std::fs;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};

/// Generates a self-signed certificate for localhost, the hostname and local
/// addresses. Returns the (cert, key) paths actually used.
pub fn ensure_tls(
    cert_path: &str,
    key_path: &str,
    auto: bool,
) -> Result<(String, String), String> {
    let cert_exists = Path::new(cert_path).exists();
    let key_exists = Path::new(key_path).exists();

    if cert_exists && key_exists {
        return Ok((cert_path.to_string(), key_path.to_string()));
    }

    if !auto {
        return Err(
            "TLS certificates not found and auto_tls is disabled. Generate them with:\n  \
             openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem \
             -days 365 -nodes"
                .to_string(),
        );
    }

    let config_dir = dirs::config_dir()
        .map(|dir| dir.join("wraith").join("tls"))
        .unwrap_or_else(|| PathBuf::from("."));

    let (cert_target, key_target) = if cert_path == "cert.pem" && key_path == "key.pem" {
        (config_dir.join("cert.pem"), config_dir.join("key.pem"))
    } else {
        (PathBuf::from(cert_path), PathBuf::from(key_path))
    };

    generate(&cert_target, &key_target)?;

    println!(
        "[wraith] generated self-signed TLS certificates at {} and {}",
        cert_target.display(),
        key_target.display()
    );

    Ok((
        cert_target.display().to_string(),
        key_target.display().to_string(),
    ))
}

pub fn generate(cert_path: &Path, key_path: &Path) -> Result<(), String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    names.extend(local_names());

    let certified = rcgen::generate_simple_self_signed(names)
        .map_err(|e| format!("failed to generate certificate: {e}"))?;

    if let Some(parent) = cert_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
    }
    if let Some(parent) = key_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
    }

    fs::write(cert_path, certified.cert.pem())
        .map_err(|e| format!("write {}: {e}", cert_path.display()))?;
    fs::write(key_path, certified.key_pair.serialize_pem())
        .map_err(|e| format!("write {}: {e}", key_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(key_path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            let _ = fs::set_permissions(key_path, permissions);
        }
    }

    Ok(())
}

fn local_names() -> Vec<String> {
    let mut names = Vec::new();

    if let Ok(hostname) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let hostname = hostname.trim();
        if !hostname.is_empty() {
            names.push(hostname.to_string());
        }
    } else if let Ok(hostname) = std::env::var("HOSTNAME") {
        if !hostname.is_empty() {
            names.push(hostname);
        }
    }

    // UDP connect does not send packets; it just resolves the local address.
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(address) = socket.local_addr() {
                let ip = address.ip().to_string();
                if !names.contains(&ip) {
                    names.push(ip);
                }
            }
        }
    }

    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generates_usable_certificate_files() {
        let dir = tempdir().expect("tempdir");
        let cert = dir.path().join("nested").join("cert.pem");
        let key = dir.path().join("nested").join("key.pem");

        generate(&cert, &key).expect("generate");

        let cert_pem = fs::read_to_string(&cert).expect("cert readable");
        let key_pem = fs::read_to_string(&key).expect("key readable");
        assert!(cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(key_pem.contains("PRIVATE KEY"));

        let (cert_used, key_used) = ensure_tls(
            &cert.display().to_string(),
            &key.display().to_string(),
            false,
        )
        .expect("existing certs are reused");
        assert_eq!(cert_used, cert.display().to_string());
        assert_eq!(key_used, key.display().to_string());
    }
}
