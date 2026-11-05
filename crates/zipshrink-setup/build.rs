//! Packs the files the installer puts on disk into `OUT_DIR/payload.bin`.
//!
//! `ZIPSHRINK_SETUP_PAYLOAD` names a folder holding exactly the files to
//! install (packaging/build-windows.sh fills it). Without it the payload is
//! empty and the installer runs as a demonstration that changes nothing,
//! which is how the interface is developed and screenshotted.
//!
//! Format: for each file, u16 name length (LE), name (UTF-8), u64 original
//! length (LE), u64 compressed length (LE), raw-deflate data.

use std::io::Write;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=ZIPSHRINK_SETUP_PAYLOAD");
    println!("cargo:rerun-if-changed=../../assets/zipshrink.ico");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("payload.bin");
    let mut blob = Vec::new();
    if let Some(dir) = std::env::var_os("ZIPSHRINK_SETUP_PAYLOAD") {
        let dir = PathBuf::from(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut names: Vec<_> =
            std::fs::read_dir(&dir).expect("payload folder").flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        names.sort();
        for p in names {
            println!("cargo:rerun-if-changed={}", p.display());
            let name = p.file_name().and_then(|n| n.to_str()).expect("UTF-8 file name").to_owned();
            let data = std::fs::read(&p).expect("read payload file");
            let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
            enc.write_all(&data).expect("compress");
            let packed = enc.finish().expect("compress");
            blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
            blob.extend_from_slice(name.as_bytes());
            blob.extend_from_slice(&(data.len() as u64).to_le_bytes());
            blob.extend_from_slice(&(packed.len() as u64).to_le_bytes());
            blob.extend_from_slice(&packed);
        }
    }
    std::fs::write(&out, blob).expect("write payload");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/zipshrink.ico")
            .set("FileDescription", "ZipShrink Installer")
            .set("ProductName", "ZipShrink")
            .set("OriginalFilename", "ZipShrink-setup.exe")
            .set("LegalCopyright", "MIT License, ZipShrink contributors")
            .set_manifest(MANIFEST);
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}

/// Runs as the user who started it (never asks for administrator rights:
/// everything goes in the user's own folders and registry), declares
/// Windows 10/11 so no compatibility shims apply, and asks for version 6 of
/// the common controls (rfd's message boxes use TaskDialog).
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="ZipShrink.Setup" version="1.0.0.0" processorArchitecture="amd64"/>
  <description>ZipShrink Installer</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;
