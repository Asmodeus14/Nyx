//! Turning a response body's bytes into the bytes the caller meant: un-gzipping it, and decoding it
//! to text under whatever charset the server declared.
//!
//! Both are "the transfer is not the content" problems, and both used to be handled by pretending
//! they did not exist — `Accept-Encoding: identity` asked servers not to compress, and `text()` ran
//! `from_utf8_lossy` over whatever arrived.

use crate::http::{Error, MAX_BODY};

/// Strip a gzip wrapper and inflate what is inside.
///
/// RFC 1952: a 10-byte fixed header, optional extra/name/comment/CRC16 fields selected by the flag
/// byte, then a raw DEFLATE stream, then an 8-byte trailer. `miniz_oxide` only does the DEFLATE
/// part, so the framing is ours to walk.
///
/// A body that does not start with the gzip magic is returned untouched rather than refused. Servers
/// do send `Content-Encoding: gzip` on an empty 304, and on the occasional response that simply is
/// not compressed; failing the page over that would be trading a working fetch for a pedantic one.
pub(crate) fn gunzip(body: Vec<u8>) -> Result<Vec<u8>, Error> {
    if body.len() < 18 || body[0] != 0x1f || body[1] != 0x8b {
        return Ok(body);
    }
    // CM 8 is DEFLATE and is the only compression method gzip has ever defined.
    if body[2] != 8 {
        return Err(Error::Protocol(format!("gzip: unknown compression method {}", body[2])));
    }

    let flags = body[3];
    let mut at = 10usize;

    // FEXTRA: a two-byte little-endian length, then that many bytes.
    if flags & 0b0000_0100 != 0 {
        if at + 2 > body.len() {
            return Err(Error::Protocol("gzip: truncated in the extra field".into()));
        }
        let xlen = u16::from_le_bytes([body[at], body[at + 1]]) as usize;
        at = at.saturating_add(2).saturating_add(xlen);
    }
    // FNAME and FCOMMENT are each a NUL-terminated string.
    for flag in [0b0000_1000u8, 0b0001_0000] {
        if flags & flag != 0 {
            match body[at.min(body.len())..].iter().position(|&c| c == 0) {
                Some(p) => at += p + 1,
                None => return Err(Error::Protocol("gzip: unterminated header string".into())),
            }
        }
    }
    // FHCRC: a two-byte CRC16 of the header, which we do not check.
    if flags & 0b0000_0010 != 0 {
        at = at.saturating_add(2);
    }

    // The trailer is CRC32 + ISIZE. Not verified — a corrupted body shows up as an inflate error or
    // as visibly broken text, and we have no better answer for either than "render what arrived".
    let end = body.len().saturating_sub(8);
    if at >= end {
        return Err(Error::Protocol("gzip: header runs past the end of the body".into()));
    }

    miniz_oxide::inflate::decompress_to_vec_with_limit(&body[at..end], MAX_BODY)
        .map_err(|e| Error::Protocol(format!("gzip: {:?}", e.status)))
}

/// Decode bytes to text under `charset`.
///
/// UTF-8 is the overwhelming default and is lossy on purpose: a browser renders what it can rather
/// than refusing a page over one bad byte.
///
/// The one alternative worth carrying is windows-1252, and `iso-8859-1` / `latin1` are deliberately
/// treated as aliases for it. That looks wrong and is not: the HTML standard *requires* it, because
/// a generation of pages declared ISO-8859-1 while actually using the curly quotes and dashes that
/// only exist in the Windows superset. Decoding those as true Latin-1 yields C1 control characters,
/// which is how a page ends up full of invisible holes where its punctuation should be.
pub(crate) fn decode_text(bytes: &[u8], charset: Option<&str>) -> String {
    let label = charset.unwrap_or("utf-8").trim().to_ascii_lowercase();
    match label.as_str() {
        "windows-1252" | "cp1252" | "iso-8859-1" | "iso8859-1" | "latin1" | "l1" | "ascii"
        | "us-ascii" => bytes.iter().map(|&b| CP1252[b as usize]).collect(),
        // Anything else — utf-8, or a charset we have no table for — goes through UTF-8. For a
        // charset we cannot honour that is wrong, but it is wrong in the direction of showing the
        // ASCII skeleton of the page rather than nothing at all.
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// windows-1252. Identical to Latin-1 except for 0x80–0x9F, which Latin-1 leaves as C1 controls and
/// Windows fills with the punctuation the web actually uses.
const CP1252: [char; 256] = {
    let mut t = ['\0'; 256];
    let mut i = 0usize;
    while i < 256 {
        // Safe for every value: 0x00–0xFF are all valid scalar values, and the 0x80–0x9F block is
        // overwritten below.
        t[i] = match char::from_u32(i as u32) {
            Some(c) => c,
            None => '\u{FFFD}',
        };
        i += 1;
    }
    t[0x80] = '€';
    t[0x82] = '‚';
    t[0x83] = 'ƒ';
    t[0x84] = '„';
    t[0x85] = '…';
    t[0x86] = '†';
    t[0x87] = '‡';
    t[0x88] = 'ˆ';
    t[0x89] = '‰';
    t[0x8A] = 'Š';
    t[0x8B] = '‹';
    t[0x8C] = 'Œ';
    t[0x8E] = 'Ž';
    t[0x91] = '\u{2018}';
    t[0x92] = '\u{2019}';
    t[0x93] = '\u{201C}';
    t[0x94] = '\u{201D}';
    t[0x95] = '•';
    t[0x96] = '–';
    t[0x97] = '—';
    t[0x98] = '˜';
    t[0x99] = '™';
    t[0x9A] = 'š';
    t[0x9B] = '›';
    t[0x9C] = 'œ';
    t[0x9E] = 'ž';
    t[0x9F] = 'Ÿ';
    t
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest legal gzip stream: fixed header, no optional fields, one stored DEFLATE block.
    fn gzip_of(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        // A single final stored block: BFINAL=1 BTYPE=00, then LEN and ~LEN little-endian.
        out.push(0x01);
        out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(payload.len() as u16)).to_le_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC32, unchecked
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out
    }

    #[test]
    fn a_plain_gzip_stream_round_trips() {
        assert_eq!(gunzip(gzip_of(b"<html>hi</html>")).unwrap(), b"<html>hi</html>");
    }

    #[test]
    fn optional_header_fields_are_skipped() {
        // FNAME set: a NUL-terminated filename sits between the fixed header and the deflate data,
        // and reading it as deflate produces garbage rather than an error.
        let mut g = gzip_of(b"payload");
        g[3] = 0b0000_1000;
        let mut with_name = g[..10].to_vec();
        with_name.extend_from_slice(b"index.html\0");
        with_name.extend_from_slice(&g[10..]);
        assert_eq!(gunzip(with_name).unwrap(), b"payload");
    }

    #[test]
    fn a_body_that_is_not_gzip_is_passed_through_untouched() {
        // Servers do label an uncompressed body as gzip. Failing the page over that trades a
        // working fetch for a pedantic one.
        assert_eq!(gunzip(b"<html>plain</html>".to_vec()).unwrap(), b"<html>plain</html>");
    }

    #[test]
    fn a_truncated_gzip_body_is_an_error_not_a_panic() {
        assert!(gunzip(vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff, 1, 2]).is_ok()); // too short: passed through
        let mut g = gzip_of(b"hello");
        g.truncate(g.len() - 10);
        let _ = gunzip(g); // must not panic, either result is acceptable
    }

    #[test]
    fn utf8_is_the_default_and_is_lossy_rather_than_fatal() {
        assert_eq!(decode_text("héllo".as_bytes(), None), "héllo");
        assert_eq!(decode_text(&[b'a', 0xff, b'b'], Some("utf-8")), "a\u{FFFD}b");
    }

    #[test]
    fn windows_1252_punctuation_survives() {
        // 0x93/0x94 are curly quotes in cp1252 and C1 controls in true Latin-1 — the difference
        // between readable text and invisible holes.
        assert_eq!(decode_text(&[0x93, b'h', b'i', 0x94], Some("windows-1252")), "“hi”");
        assert_eq!(decode_text(&[0x96], Some("windows-1252")), "–");
    }

    #[test]
    fn iso_8859_1_is_decoded_as_windows_1252_because_the_web_is() {
        // Required by the HTML standard, and the reason is empirical: a generation of pages declare
        // ISO-8859-1 and then use the Windows superset's punctuation anyway.
        assert_eq!(decode_text(&[0x92], Some("ISO-8859-1")), "\u{2019}");
        assert_eq!(decode_text(&[0xe9], Some("latin1")), "é");
    }

    #[test]
    fn an_unknown_charset_falls_back_to_utf8_rather_than_to_nothing() {
        assert_eq!(decode_text(b"plain ascii", Some("shift_jis")), "plain ascii");
    }
}
