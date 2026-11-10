//! Just enough HTTP/1.1 for the web demos: the request line, query
//! parameters, `#rrggbb` colors and response headers.
//!
//! No networking here, so it is unit-tested on a PC; the demos read the
//! request from a TCP socket and write the response back.

use core::fmt::Write;

/// The parts of a request line (`GET /api/led?color=ff0000 HTTP/1.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request<'a> {
    /// `GET`, `POST`, ...
    pub method: &'a str,
    /// The path without the query, e.g. `/api/led`.
    pub path: &'a str,
    /// The part after `?`, empty if there is none.
    pub query: &'a str,
}

/// Parses the request line at the start of `head` (the bytes received so
/// far). `None` if the line is incomplete or malformed.
#[must_use]
pub fn parse_request(head: &[u8]) -> Option<Request<'_>> {
    let end = head.windows(2).position(|w| w == b"\r\n")?;
    let line = core::str::from_utf8(&head[..end]).ok()?;
    let mut parts = line.split(' ');
    let method = parts.next().filter(|m| !m.is_empty())?;
    let target = parts.next()?;
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Some(Request {
        method,
        path,
        query,
    })
}

/// Whether `head` holds the complete request header (ends with a blank line).
#[must_use]
pub fn header_complete(head: &[u8]) -> bool {
    head.windows(4).any(|w| w == b"\r\n\r\n")
}

/// The value of `key` in a query string like `a=1&color=ff0000`.
#[must_use]
pub fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// Parses `rrggbb` (hex, with or without a leading `#` or its URL encoding
/// `%23`) into red, green and blue.
#[must_use]
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let hex = s
        .strip_prefix('#')
        .or_else(|| s.strip_prefix("%23"))
        .unwrap_or(s);
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Writes formatted text into a byte buffer; fails when it is full.
struct Cursor<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Write for Cursor<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        let dst = self.buf.get_mut(self.len..end).ok_or(core::fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Formats into `buf` and returns the written bytes, or `None` if `buf` is
/// too small.
pub fn format<'a>(buf: &'a mut [u8], args: core::fmt::Arguments<'_>) -> Option<&'a [u8]> {
    let mut cursor = Cursor { buf, len: 0 };
    cursor.write_fmt(args).ok()?;
    let len = cursor.len;
    Some(&buf[..len])
}

/// The header of a response with a `body_len`-byte body; the connection is
/// closed after it.
pub fn response_header<'a>(
    buf: &'a mut [u8],
    status: &str,
    content_type: &str,
    body_len: usize,
) -> Option<&'a [u8]> {
    format(
        buf,
        format_args!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n\
             Content-Length: {body_len}\r\nCache-Control: no-store\r\n\
             Connection: close\r\n\r\n"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_line() {
        let req = parse_request(b"GET /api/led?color=ff8000 HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/api/led");
        assert_eq!(req.query, "color=ff8000");

        let req = parse_request(b"GET / HTTP/1.0\r\n").unwrap();
        assert_eq!((req.path, req.query), ("/", ""));
    }

    #[test]
    fn bad_request_lines() {
        assert_eq!(parse_request(b"GET / HTTP/1.1"), None); // incomplete
        assert_eq!(parse_request(b"GET /\r\n"), None);
        assert_eq!(parse_request(b" / HTTP/1.1\r\n"), None);
        assert_eq!(parse_request(b"GET / SPDY/3\r\n"), None);
    }

    #[test]
    fn header_end() {
        assert!(!header_complete(b"GET / HTTP/1.1\r\nHost: x\r\n"));
        assert!(header_complete(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"));
    }

    #[test]
    fn query_params() {
        assert_eq!(query_param("color=ff0000", "color"), Some("ff0000"));
        assert_eq!(query_param("a=1&color=00ff00&b", "color"), Some("00ff00"));
        assert_eq!(query_param("a=1", "color"), None);
        assert_eq!(query_param("", "color"), None);
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_color("#00FF7f"), Some([0, 255, 127]));
        assert_eq!(parse_color("%23000001"), Some([0, 0, 1]));
        assert_eq!(parse_color("ff80"), None);
        assert_eq!(parse_color("gg0000"), None);
        assert_eq!(parse_color("ffé00"), None);
    }

    #[test]
    fn header() {
        let mut buf = [0u8; 160];
        let h = response_header(&mut buf, "200 OK", "text/html", 42).unwrap();
        let h = core::str::from_utf8(h).unwrap();
        assert!(h.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(h.contains("Content-Length: 42\r\n"));
        assert!(h.ends_with("\r\n\r\n"));

        let mut small = [0u8; 10];
        assert!(response_header(&mut small, "200 OK", "text/html", 42).is_none());
    }
}
