//! RFC 3164 syslog writer for the `syslog` log target. `SyslogMakeWriter` hands
//! `tracing-subscriber` one `SyslogWriter` per event; each buffers the formatted line and
//! sends it as a single `/dev/log` datagram on drop, with a priority derived from the event's
//! level.
use std::io;
use std::os::unix::net::UnixDatagram;
use std::sync::Arc;

use tracing_subscriber::fmt::MakeWriter;

/// RFC 3164 facility code for "user-level messages", the conventional facility for
/// applications that aren't a system daemon.
const FACILITY_USER: u8 = 1;

#[derive(Clone)]
pub(super) struct SyslogMakeWriter {
    pub(super) socket: Arc<UnixDatagram>,
}

impl<'a> MakeWriter<'a> for SyslogMakeWriter {
    type Writer = SyslogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.writer_for_severity(6) // informational
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        self.writer_for_severity(severity_for_level(*meta.level()))
    }
}

/// Maps a tracing level to its RFC 5424 severity number.
fn severity_for_level(level: tracing::Level) -> u8 {
    match level {
        tracing::Level::ERROR => 3,
        tracing::Level::WARN => 4,
        tracing::Level::INFO => 6,
        tracing::Level::DEBUG | tracing::Level::TRACE => 7,
    }
}

impl SyslogMakeWriter {
    fn writer_for_severity(&self, severity: u8) -> SyslogWriter {
        SyslogWriter {
            socket: self.socket.clone(),
            severity,
            buf: Vec::new(),
        }
    }
}

/// One instance is created per log event (via `make_writer_for`) and dropped right after
/// `tracing-subscriber` finishes formatting into it. Buffering until that drop, rather than
/// sending on every `write` call, guarantees the whole formatted line goes out as a single
/// syslog datagram instead of being split across several.
pub(super) struct SyslogWriter {
    socket: Arc<UnixDatagram>,
    severity: u8,
    buf: Vec<u8>,
}

impl io::Write for SyslogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let pri = FACILITY_USER * 8 + self.severity;
        let mut datagram = format!("<{pri}>yorishiro-server: ").into_bytes();
        datagram.extend_from_slice(&self.buf);
        self.socket.send(&datagram)?;
        self.buf.clear();
        Ok(())
    }
}

impl Drop for SyslogWriter {
    fn drop(&mut self) {
        let _ = io::Write::flush(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syslog_writer_sends_one_datagram_per_dropped_writer_with_the_right_pri() {
        let (client, server) = UnixDatagram::pair().unwrap();
        let make_writer = SyslogMakeWriter {
            socket: Arc::new(client),
        };

        {
            let mut writer = make_writer.writer_for_severity(6);
            // Two separate `write` calls (as tracing-subscriber issues for a formatted line
            // plus its trailing newline) must still coalesce into a single datagram.
            io::Write::write_all(&mut writer, b"{\"message\":\"hello\"}").unwrap();
            io::Write::write_all(&mut writer, b"\n").unwrap();
        } // dropped here, which flushes

        let mut buf = [0u8; 256];
        let n = server.recv(&mut buf).unwrap();
        let received = std::str::from_utf8(&buf[..n]).unwrap();

        // facility (user, 1) * 8 + severity (informational, 6) = 14
        assert_eq!(received, "<14>yorishiro-server: {\"message\":\"hello\"}\n");
    }

    #[test]
    fn severity_for_level_matches_rfc_5424() {
        assert_eq!(severity_for_level(tracing::Level::ERROR), 3);
        assert_eq!(severity_for_level(tracing::Level::WARN), 4);
        assert_eq!(severity_for_level(tracing::Level::INFO), 6);
        assert_eq!(severity_for_level(tracing::Level::DEBUG), 7);
        assert_eq!(severity_for_level(tracing::Level::TRACE), 7);
    }

    #[test]
    fn writer_for_severity_frames_the_pri_correctly_for_an_error_level() {
        let (client, server) = UnixDatagram::pair().unwrap();
        let make_writer = SyslogMakeWriter {
            socket: Arc::new(client),
        };

        {
            let mut writer =
                make_writer.writer_for_severity(severity_for_level(tracing::Level::ERROR));
            io::Write::write_all(&mut writer, b"boom").unwrap();
        }

        let mut buf = [0u8; 256];
        let n = server.recv(&mut buf).unwrap();
        let received = std::str::from_utf8(&buf[..n]).unwrap();

        // facility (user, 1) * 8 + severity (error, 3) = 11
        assert_eq!(received, "<11>yorishiro-server: boom");
    }

    #[test]
    fn flushing_an_empty_buffer_sends_nothing() {
        let (client, server) = UnixDatagram::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        let make_writer = SyslogMakeWriter {
            socket: Arc::new(client),
        };

        drop(make_writer.writer_for_severity(6));

        let mut buf = [0u8; 16];
        assert!(
            server.recv(&mut buf).is_err(),
            "expected no datagram to arrive"
        );
    }
}
