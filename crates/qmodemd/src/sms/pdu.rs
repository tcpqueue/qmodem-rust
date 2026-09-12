//! 3GPP TS 23.040 TPDU and TS 23.038 alphabets. No shell or external SMS process.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
const ALPHABET: &str = "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞ\u{1b}ÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà";
fn extension(code: u8) -> Option<char> {
    Some(match code {
        10 => '\u{000c}',
        20 => '^',
        40 => '{',
        41 => '}',
        47 => '\\',
        60 => '[',
        61 => '~',
        62 => ']',
        64 => '|',
        101 => '€',
        _ => return None,
    })
}
fn septets(text: &str) -> Option<Vec<u8>> {
    let mut result = vec![];
    for ch in text.chars() {
        if let Some(i) = ALPHABET.chars().position(|c| c == ch && c != '\u{1b}') {
            result.push(i as u8);
        } else {
            let i = (0..128).find(|i| extension(*i) == Some(ch))?;
            result.extend([27, i]);
        }
    }
    Some(result)
}
fn pack(data: &[u8], header: &[u8]) -> Vec<u8> {
    let offset = (header.len() * 8).div_ceil(7) * 7;
    let mut out = vec![0u8; (offset + data.len() * 7).div_ceil(8)];
    out[..header.len()].copy_from_slice(header);
    for (i, &s) in data.iter().enumerate() {
        let bit = offset + i * 7;
        out[bit / 8] |= s << (bit % 8);
        if bit % 8 > 1 {
            out[bit / 8 + 1] |= s >> (8 - bit % 8);
        }
    }
    out
}
fn unpack(data: &[u8], count: usize, offset: usize) -> Result<String> {
    ensure!(offset + count * 7 <= data.len() * 8, "truncated GSM7 data");
    let alphabet = ALPHABET.chars().collect::<Vec<_>>();
    let mut out = String::new();
    let mut escaped = false;
    for i in 0..count {
        let bit = offset + i * 7;
        let low = (data[bit / 8] as u16) >> (bit % 8);
        let high = data.get(bit / 8 + 1).copied().unwrap_or(0) as u16;
        let s = ((low | (high << (8 - bit % 8))) & 127) as u8;
        if escaped {
            out.push(extension(s).unwrap_or('�'));
            escaped = false;
        } else if s == 27 {
            escaped = true;
        } else {
            out.push(alphabet[s as usize]);
        }
    }
    if escaped {
        out.push('�');
    }
    Ok(out)
}
pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect()
}
fn unhex(text: &str) -> Result<Vec<u8>> {
    ensure!(
        text.len().is_multiple_of(2)
            && text.len() <= 4096
            && text.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid PDU hex"
    );
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p)?, 16).context("hex octet"))
        .collect()
}
fn number(number: &str) -> Result<(u8, Vec<u8>)> {
    let international = number.starts_with('+');
    let digits = number.trim_start_matches('+');
    ensure!(
        !digits.is_empty() && digits.len() <= 20 && digits.bytes().all(|b| b.is_ascii_digit()),
        "recipient must contain 1 to 20 digits and an optional leading +"
    );
    let data = digits
        .as_bytes()
        .chunks(2)
        .map(|pair| (pair[0] - b'0') | ((pair.get(1).map(|c| c - b'0').unwrap_or(15)) << 4))
        .collect();
    Ok((if international { 0x91 } else { 0x81 }, data))
}
#[derive(Debug, Clone, Serialize)]
pub struct Encoded {
    pub pdu: String,
    pub tpdu_length: usize,
    pub part: u8,
    pub total: u8,
    pub encoding: &'static str,
}
pub fn encode(peer: &str, text: &str, reference: u8) -> Result<Vec<Encoded>> {
    ensure!(
        !text.is_empty() && text.len() <= 32 * 1024,
        "SMS text must be 1 to 32768 bytes"
    );
    let (toa, address) = number(peer)?;
    let gsm = septets(text);
    let mut pieces: Vec<Vec<u8>> = vec![];
    let is_gsm = gsm.is_some();
    if let Some(gsm) = gsm {
        let width = if gsm.len() <= 160 { 160 } else { 153 };
        let mut start = 0;
        while start < gsm.len() {
            let mut end = (start + width).min(gsm.len());
            if end < gsm.len() && gsm[end - 1] == 27 {
                end -= 1;
            }
            pieces.push(gsm[start..end].to_vec());
            start = end;
        }
    } else {
        let width = if text.encode_utf16().count() <= 70 {
            140
        } else {
            134
        };
        let mut piece = vec![];
        for ch in text.chars() {
            let mut buf = [0; 2];
            let units = ch.encode_utf16(&mut buf);
            if piece.len() + units.len() * 2 > width {
                pieces.push(std::mem::take(&mut piece));
            }
            for u in units {
                piece.extend(u.to_be_bytes());
            }
        }
        if !piece.is_empty() {
            pieces.push(piece);
        }
    }
    ensure!(pieces.len() <= 32, "SMS exceeds 32 parts");
    let total = pieces.len() as u8;
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, piece)| {
            let header = if total > 1 {
                vec![5, 0, 3, reference, total, i as u8 + 1]
            } else {
                vec![]
            };
            let user_data = if is_gsm {
                pack(&piece, &header)
            } else {
                [header.clone(), piece.clone()].concat()
            };
            let udl = if is_gsm {
                piece.len() + (header.len() * 8).div_ceil(7)
            } else {
                user_data.len()
            };
            let mut pdu = vec![
                0,
                if total > 1 { 0x51 } else { 0x11 },
                0,
                peer.trim_start_matches('+').len() as u8,
                toa,
            ];
            pdu.extend(&address);
            pdu.extend([0, if is_gsm { 0 } else { 8 }, 0xff, udl as u8]);
            pdu.extend(user_data);
            Ok(Encoded {
                tpdu_length: pdu.len() - 1,
                pdu: hex(&pdu),
                part: i as u8 + 1,
                total,
                encoding: if is_gsm { "gsm7" } else { "ucs2" },
            })
        })
        .collect()
}
struct Reader {
    data: Vec<u8>,
    pos: usize,
}
impl Reader {
    fn byte(&mut self) -> Result<u8> {
        let value = *self.data.get(self.pos).context("truncated TPDU")?;
        self.pos += 1;
        Ok(value)
    }
    fn take(&mut self, n: usize) -> Result<Vec<u8>> {
        let end = self.pos.checked_add(n).context("TPDU length overflow")?;
        let bytes = self
            .data
            .get(self.pos..end)
            .context("truncated TPDU")?
            .to_vec();
        self.pos = end;
        Ok(bytes)
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Decoded {
    pub peer: String,
    pub content: String,
    pub timestamp: Option<i64>,
    pub encoding: String,
    pub concat: Option<Concat>,
    pub direction: &'static str,
    pub binary: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Concat {
    pub reference: u16,
    pub total: u8,
    pub part: u8,
}
fn decode_number(r: &mut Reader) -> Result<String> {
    let count = r.byte()? as usize;
    let toa = r.byte()?;
    let raw = r.take(count.div_ceil(2))?;
    if toa & 0x70 == 0x50 {
        return unpack(&raw, count * 4 / 7, 0);
    }
    let mut digits = String::new();
    if toa & 0x70 == 0x10 {
        digits.push('+');
    }
    for (i, n) in raw
        .iter()
        .flat_map(|b| [b & 15, b >> 4])
        .take(count)
        .enumerate()
    {
        ensure!(n <= 9, "invalid address nibble at {i}");
        digits.push(char::from(b'0' + n));
    }
    Ok(digits)
}
fn date(bytes: &[u8]) -> Option<i64> {
    use chrono::{FixedOffset, NaiveDate, TimeZone};
    let bcd =
        |b: u8| (b & 15 <= 9 && b >> 4 <= 9).then_some((b & 15) as u32 * 10 + (b >> 4) as u32);
    let year = bcd(bytes[0])?;
    let year = if year >= 70 { 1900 + year } else { 2000 + year };
    let date = NaiveDate::from_ymd_opt(year as i32, bcd(bytes[1])?, bcd(bytes[2])?)?.and_hms_opt(
        bcd(bytes[3])?,
        bcd(bytes[4])?,
        bcd(bytes[5])?,
    )?;
    let negative = bytes[6] & 8 != 0;
    let offset = bcd(bytes[6] & !8)? as i32 * 900;
    FixedOffset::east_opt(if negative { -offset } else { offset })?
        .from_local_datetime(&date)
        .single()
        .map(|d| d.timestamp())
}
pub fn decode(text: &str) -> Result<Decoded> {
    let mut r = Reader {
        data: unhex(text)?,
        pos: 0,
    };
    let smsc = r.byte()? as usize;
    r.take(smsc)?;
    let first = r.byte()?;
    let kind = first & 3;
    ensure!(kind <= 1, "unsupported TPDU type: {kind}");
    if kind == 1 {
        r.byte()?;
    }
    let peer = decode_number(&mut r)?;
    r.byte()?;
    let dcs = r.byte()?;
    let timestamp = if kind == 0 {
        date(&r.take(7)?)
    } else {
        match (first >> 3) & 3 {
            0 => {}
            2 => {
                r.take(1)?;
            }
            _ => {
                r.take(7)?;
            }
        }
        None
    };
    let udl = r.byte()? as usize;
    let coding = if dcs & 0xc0 == 0 {
        ensure!(dcs & 0x20 == 0, "compressed SMS is unsupported");
        (dcs >> 2) & 3
    } else if dcs & 0xf0 == 0xe0 {
        2
    } else if dcs & 0xf0 == 0xf0 {
        if dcs & 4 == 0 { 0 } else { 1 }
    } else {
        0
    };
    ensure!(coding != 3, "reserved SMS alphabet");
    let raw = r.take(if coding == 0 {
        (udl * 7).div_ceil(8)
    } else {
        udl
    })?;
    let mut header = 0;
    let mut concat = None;
    if first & 0x40 != 0 {
        header = raw.first().copied().context("missing UDH")? as usize + 1;
        ensure!(header <= raw.len(), "truncated UDH");
        let mut pos = 1;
        while pos < header {
            ensure!(pos + 2 <= header, "truncated UDH element");
            let kind = raw[pos];
            let len = raw[pos + 1] as usize;
            pos += 2;
            ensure!(pos + len <= header, "UDH element exceeds header");
            let data = &raw[pos..pos + len];
            match (kind, len) {
                (0, 3) => {
                    concat = Some(Concat {
                        reference: data[0] as u16,
                        total: data[1],
                        part: data[2],
                    });
                }
                (8, 4) => {
                    concat = Some(Concat {
                        reference: u16::from_be_bytes([data[0], data[1]]),
                        total: data[2],
                        part: data[3],
                    });
                }
                _ => {}
            }
            pos += len;
        }
    }
    if let Some(c) = &concat {
        ensure!(
            c.total > 0 && c.part > 0 && c.part <= c.total,
            "invalid multipart numbering"
        );
    }
    let (content, binary, encoding) = match coding {
        0 => {
            let skip = (header * 8).div_ceil(7);
            ensure!(udl >= skip, "UDH exceeds GSM7 length");
            (unpack(&raw, udl - skip, skip * 7)?, None, "gsm7")
        }
        1 => (String::new(), Some(hex(&raw[header..])), "8bit"),
        2 => {
            ensure!((raw.len() - header).is_multiple_of(2), "odd UCS2 payload");
            let units = raw[header..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_be_bytes([p[0], p[1]]))
                .collect::<Vec<_>>();
            (
                String::from_utf16(&units).context("invalid UCS2 surrogate sequence")?,
                None,
                "ucs2",
            )
        }
        _ => bail!("unsupported alphabet"),
    };
    Ok(Decoded {
        peer,
        content,
        timestamp,
        encoding: encoding.into(),
        concat,
        direction: if kind == 0 { "received" } else { "sent" },
        binary,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gsm7_known_vector() {
        assert_eq!(
            hex(&pack(&septets("hellohello").unwrap(), &[])),
            "E8329BFD4697D9EC37"
        );
        assert_eq!(
            unpack(&unhex("E8329BFD4697D9EC37").unwrap(), 10, 0).unwrap(),
            "hellohello"
        );
    }
    #[test]
    fn submit_roundtrip_preserves_extensions_and_unicode() {
        for text in ["Hello @£ € {}[]^~\\|", "你好，短信。😀"] {
            let encoded = encode("+8613800000000", text, 42).unwrap();
            assert_eq!(encoded.len(), 1);
            let decoded = decode(&encoded[0].pdu).unwrap();
            assert_eq!(decoded.content, text);
            assert_eq!(decoded.peer, "+8613800000000");
        }
    }
    #[test]
    fn multipart_never_splits_escape_or_surrogate() {
        for text in ["^".repeat(200), "短信😀".repeat(60), "A".repeat(500)] {
            let parts = encode("10086", &text, 42).unwrap();
            assert!(parts.len() > 1);
            let decoded = parts
                .iter()
                .map(|p| decode(&p.pdu).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                decoded
                    .iter()
                    .map(|d| d.content.as_str())
                    .collect::<String>(),
                text
            );
            for (i, d) in decoded.iter().enumerate() {
                let c = d.concat.as_ref().unwrap();
                assert_eq!(c.part as usize, i + 1);
                assert_eq!(c.total as usize, parts.len());
            }
        }
    }
    #[test]
    fn malformed_lengths_fail_without_panics() {
        for text in ["", "0", "00", "0004FF", "000491", "XX"] {
            assert!(decode(text).is_err());
        }
    }
}
