//! D-Bus introspection XML generator.
//!
//! Exports `VerbisageDbus` over a P2P Unix socket pair, introspects it, and
//! writes the `org.verbisage.Dictionary1` interface description as a standalone
//! XML file.
//!
//! Usage:
//!   verbisage-introspect [OUTPUT]
//!
//! If OUTPUT is omitted the XML is written to stdout.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use zbus::blocking::connection::Builder as ConnectionBuilder;

use verbisage::daemon::DaemonHandler;
use verbisage::daemon::config::DaemonConfig;
use verbisage::daemon::dbus::VerbisageDbus;

const P2P_GUID: &str = "0123456789abcdef0123456789abcdef";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let output_path = args.get(1).map(PathBuf::from);

    let handler = DaemonHandler::with_config(DaemonConfig::default_for("en_US"));
    let dbus_obj = VerbisageDbus::new(handler);

    let (s1, s2) = UnixStream::pair().unwrap_or_else(|e| {
        eprintln!("error: failed to create Unix socket pair: {}", e);
        std::process::exit(1);
    });

    // Server: blocking connection on a background thread.
    thread::spawn(move || {
        let result = ConnectionBuilder::unix_stream(s1).p2p().server(P2P_GUID);
        let result = match result {
            Err(e) => {
                eprintln!("error: {}", e);
                return;
            }
            Ok(b) => b,
        };
        let result = result.serve_at("/org/verbisage/Dictionary", dbus_obj);
        let result = match result {
            Err(e) => {
                eprintln!("error: {}", e);
                return;
            }
            Ok(b) => b,
        };
        let server = result.build();
        match server {
            Err(e) => eprintln!("error: failed to build D-Bus server: {}", e),
            Ok(_conn) => loop {
                thread::sleep(Duration::from_secs(3600));
            },
        }
    });

    thread::sleep(Duration::from_millis(200));

    let client = ConnectionBuilder::unix_stream(s2)
        .p2p()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to build D-Bus client: {}", e);
            std::process::exit(1);
        });

    // Raw method call — no proxy needed for P2P.
    let msg = client
        .call_method(
            None::<&str>, // P2P: no destination
            "/org/verbisage/Dictionary",
            Some("org.freedesktop.DBus.Introspectable"),
            "Introspect",
            &(),
        )
        .unwrap_or_else(|e| {
            eprintln!("error: introspection call failed: {}", e);
            std::process::exit(1);
        });

    let raw_xml: String = msg.body().deserialize().unwrap_or_else(|e| {
        eprintln!("error: failed to deserialize introspection response: {}", e);
        std::process::exit(1);
    });

    // Extract the interface block.
    let marker = r#"<interface name="org.verbisage.Dictionary1""#;
    let start = raw_xml.find(marker).unwrap_or_else(|| {
        eprintln!("error: interface 'org.verbisage.Dictionary1' not found in introspection output");
        eprintln!("--- raw output ---\n{}", raw_xml);
        std::process::exit(1);
    });
    let end = raw_xml[start..]
        .find("</interface>")
        .map(|i| i + start + "</interface>".len())
        .unwrap_or(raw_xml.len());

    let interface_xml = &raw_xml[start..end];

    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
{}
</node>
"#,
        interface_xml
    );

    if let Some(path) = &output_path {
        std::fs::write(path, &doc).unwrap_or_else(|e| {
            eprintln!("error: failed to write '{}': {}", path.display(), e);
            std::process::exit(1);
        });
    } else {
        print!("{}", doc);
    }

    std::process::exit(0);
}
