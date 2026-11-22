use std::net::IpAddr;
use std::path::Path;

use chrono::{Datelike, Duration, Utc};
use rcgen::{
    date_time_ymd, BasicConstraints, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType,
};
use thiserror::Error;

/// Apple iOS 13+ / Safari rejects TLS leaf certificates valid for > 825 days (<= 398 days recommended).
pub const LEAF_CERT_VALIDITY_DAYS: i64 = 365;
pub const CA_CERT_VALIDITY_DAYS: i64 = 3650;

#[derive(Debug, Error)]
pub enum CertError {
    #[error("Certificate generation error: {0}")]
    Rcgen(#[from] rcgen::Error),

    #[error("I/O error persisting local certificates: {0}")]
    Io(#[from] std::io::Error),
}

/// Persisted Local Root CA + leaf HTTPS server certificate PEM bundle.
#[derive(Debug, Clone)]
pub struct LocalTlsBundle {
    pub ca_cert_pem: String,
    pub server_cert_pem: String,
    pub server_key_pem: String,
}

/// Ensures a persistent Local Root CA (`Karaok Party Local CA`) exists in `certs_dir`
/// and signs a fresh 365-day iOS-compliant server leaf certificate covering `karaok-party.local`,
/// `localhost`, `127.0.0.1`, and the active LAN IP address.
pub fn ensure_local_tls_certs(certs_dir: &Path, lan_ip: IpAddr) -> Result<LocalTlsBundle, CertError> {
    std::fs::create_dir_all(certs_dir)?;
    let ca_cert_path = certs_dir.join("karaok-party-ca.pem");
    let ca_key_path = certs_dir.join("karaok-party-ca.key");

    // 1. Load existing Local Root CA (preserving exact persisted CA cert metadata) or generate fresh
    let (ca_cert, ca_key, ca_cert_pem) = if ca_cert_path.exists() && ca_key_path.exists() {
        let existing_ca_pem = std::fs::read_to_string(&ca_cert_path)?;
        let key_pem = std::fs::read_to_string(&ca_key_path)?;
        match (
            CertificateParams::from_ca_cert_pem(&existing_ca_pem),
            KeyPair::from_pem(&key_pem),
        ) {
            (Ok(ca_params), Ok(ca_key)) => {
                let ca_cert = ca_params.self_signed(&ca_key)?;
                (ca_cert, ca_key, existing_ca_pem)
            }
            _ => generate_and_persist_ca(&ca_cert_path, &ca_key_path)?,
        }
    } else {
        generate_and_persist_ca(&ca_cert_path, &ca_key_path)?
    };

    // 2. Always sign a fresh iOS-compliant 365-day leaf cert for the active LAN IP + karaok-party.local
    let now = Utc::now();
    let nb = now - Duration::days(1);
    let na = now + Duration::days(LEAF_CERT_VALIDITY_DAYS);

    let mut server_params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "karaok-party.local");
    dn.push(DnType::OrganizationName, "Karaok Party Studio");
    server_params.distinguished_name = dn;
    server_params.is_ca = IsCa::NoCa;
    server_params.not_before = date_time_ymd(nb.year(), nb.month() as u8, nb.day() as u8);
    server_params.not_after = date_time_ymd(na.year(), na.month() as u8, na.day() as u8);
    server_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.subject_alt_names = vec![
        SanType::DnsName("karaok-party.local".try_into()?),
        SanType::DnsName("localhost".try_into()?),
        SanType::IpAddress(lan_ip),
        SanType::IpAddress(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
    ];

    let server_key = KeyPair::generate()?;
    let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key)?;
    let server_cert_pem = format!("{}\n{}", server_cert.pem(), ca_cert_pem);
    let server_key_pem = server_key.serialize_pem();

    let server_cert_path = certs_dir.join("server.pem");
    let server_key_path = certs_dir.join("server.key");
    std::fs::write(&server_cert_path, &server_cert_pem)?;
    write_private_key_file(&server_key_path, &server_key_pem)?;

    Ok(LocalTlsBundle {
        ca_cert_pem,
        server_cert_pem,
        server_key_pem,
    })
}

fn generate_and_persist_ca(
    ca_cert_path: &Path,
    ca_key_path: &Path,
) -> Result<(rcgen::Certificate, KeyPair, String), CertError> {
    let ca_key = KeyPair::generate()?;
    let ca_params = build_ca_params();
    let ca_cert = ca_params.self_signed(&ca_key)?;
    let cert_pem = ca_cert.pem();
    std::fs::write(ca_cert_path, &cert_pem)?;
    write_private_key_file(ca_key_path, &ca_key.serialize_pem())?;
    Ok((ca_cert, ca_key, cert_pem))
}

fn build_ca_params() -> CertificateParams {
    let now = Utc::now();
    let nb = now - Duration::days(1);
    let na = now + Duration::days(CA_CERT_VALIDITY_DAYS);

    let mut ca_params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Karaok Party Local CA");
    dn.push(DnType::OrganizationName, "Karaok Party Open Source");
    ca_params.distinguished_name = dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.not_before = date_time_ymd(nb.year(), nb.month() as u8, nb.day() as u8);
    ca_params.not_after = date_time_ymd(na.year(), na.month() as u8, na.day() as u8);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params
}

fn write_private_key_file(path: &Path, pem: &str) -> Result<(), std::io::Error> {
    std::fs::write(path, pem)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn generates_persists_ios_compliant_certs_with_0600_key_permissions() {
        let dir = std::env::temp_dir().join(format!("karaok-certs-test-{}", uuid::Uuid::new_v4()));
        let bundle =
            ensure_local_tls_certs(&dir, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42))).unwrap();
        assert!(bundle.ca_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(bundle.server_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(bundle.server_key_pem.contains("BEGIN PRIVATE KEY"));

        // Compile-time guarantee: leaf cert validity window is <= 398 days (Apple iOS limit is 825 days)
        const { assert!(LEAF_CERT_VALIDITY_DAYS <= 398) };

        // Verify private keys are chmod 0600 on macOS / Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ca_mode = std::fs::metadata(dir.join("karaok-party-ca.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let srv_mode = std::fs::metadata(dir.join("server.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(ca_mode, 0o600);
            assert_eq!(srv_mode, 0o600);
        }

        // Second invocation reuses exact persisted CA cleanly
        let bundle2 =
            ensure_local_tls_certs(&dir, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42))).unwrap();
        assert_eq!(bundle.ca_cert_pem, bundle2.ca_cert_pem);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
