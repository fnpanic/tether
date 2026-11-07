//! Custom apps for macOS (docs/api.md "Custom apps"): copy the TetherQS release zip with its one
//! top-level `.app` folder renamed. Entry data, CRCs, modes and the code signature inside stay
//! byte-for-byte the same (the folder name is not part of the signature); only the names in the
//! local and central headers change, and the offsets move with them. Streams file to file, so the
//! 64 MB service never holds the zip in memory.

use std::io::{self, Read, Seek, SeekFrom, Write};

const EOCD: u32 = 0x0605_4b50;
const CEN: u32 = 0x0201_4b50;
const LOC: u32 = 0x0403_4b50;
/// End-of-central-directory record plus the longest zip comment.
const TAIL: u64 = 22 + 65_535;
const MAX_CD: u64 = 16 << 20;

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("release zip: {msg}"))
}

/// `src` (a `ditto -c -k --keepParent X.app` zip) → `out` with `X.app` renamed to `new_app`
/// (also in `__MACOSX/`). Zip64 and archives with anything but one `.app` at the top are refused.
pub fn rename_app<R: Read + Seek, W: Write>(src: &mut R, out: &mut W, new_app: &str) -> io::Result<u64> {
    if new_app.contains('/') || !new_app.ends_with(".app") {
        return Err(bad("invalid bundle name"));
    }
    let len = src.seek(SeekFrom::End(0))?;
    let start = len.saturating_sub(TAIL);
    src.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    src.read_to_end(&mut tail)?;
    let e = (0..tail.len().saturating_sub(21)).rev().find(|&i| u32le(&tail, i) == EOCD).ok_or_else(|| bad("no end record"))?;
    let eocd = tail[e..].to_vec();
    let (n, cd_size, cd_off) = (u16le(&eocd, 10) as usize, u32le(&eocd, 12) as u64, u32le(&eocd, 16) as u64);
    if n == 0xffff || cd_size == 0xffff_ffff || cd_off == 0xffff_ffff || u16le(&eocd, 4) != 0 || u16le(&eocd, 8) as usize != n {
        return Err(bad("zip64 or multi-disk archives are not supported"));
    }
    if cd_size > MAX_CD || cd_off + cd_size != start + e as u64 {
        return Err(bad("central directory does not end at the end record"));
    }
    src.seek(SeekFrom::Start(cd_off))?;
    let mut cd = vec![0; cd_size as usize];
    src.read_exact(&mut cd)?;

    // (central record range, local header offset, name)
    let mut ents = Vec::with_capacity(n);
    let mut p = 0;
    for _ in 0..n {
        if p + 46 > cd.len() || u32le(&cd, p) != CEN {
            return Err(bad("damaged central directory"));
        }
        let (nl, xl, cl) = (u16le(&cd, p + 28) as usize, u16le(&cd, p + 30) as usize, u16le(&cd, p + 32) as usize);
        let end = p + 46 + nl + xl + cl;
        if end > cd.len() {
            return Err(bad("damaged central directory"));
        }
        ents.push((p..end, u32le(&cd, p + 42) as u64, cd[p + 46..p + 46 + nl].to_vec()));
        p = end;
    }
    let top = ents.iter().find(|e| !e.2.starts_with(b"__MACOSX/")).and_then(|e| e.2.split(|&b| b == b'/').next()).map(<[u8]>::to_vec).ok_or_else(|| bad("empty"))?;
    if !top.ends_with(b".app") {
        return Err(bad("no .app at the top"));
    }
    let new = new_app.as_bytes();
    let rename = |name: &[u8]| -> io::Result<Vec<u8>> {
        for (head, tail) in [(&b""[..], &b"/"[..]), (b"__MACOSX/", b"/"), (b"__MACOSX/._", b"")] {
            if let Some(rest) = name.strip_prefix([head, &top, tail].concat().as_slice()) {
                return Ok([head, new, tail, rest].concat());
            }
        }
        Err(bad("entry outside the .app"))
    };

    // Local entries in file order; each runs up to the next one (data, data descriptor included).
    let mut offs: Vec<u64> = ents.iter().map(|e| e.1).collect();
    offs.sort_unstable();
    offs.dedup();
    if offs.first() != Some(&0) {
        return Err(bad("data before the first entry"));
    }
    offs.push(cd_off);
    let mut moved = std::collections::HashMap::new();
    let mut pos = 0u64;
    for w in offs.windows(2) {
        let (at, next) = (w[0], w[1]);
        src.seek(SeekFrom::Start(at))?;
        let mut h = [0u8; 30];
        src.read_exact(&mut h)?;
        let nl = u16le(&h, 26) as u64;
        if u32le(&h, 0) != LOC || at + 30 + nl > next {
            return Err(bad("damaged local header"));
        }
        let mut name = vec![0; nl as usize];
        src.read_exact(&mut name)?;
        let nn = rename(&name)?;
        h[26..28].copy_from_slice(&(nn.len() as u16).to_le_bytes());
        out.write_all(&h)?;
        out.write_all(&nn)?;
        let rest = next - at - 30 - nl;
        if io::copy(&mut src.by_ref().take(rest), out)? != rest {
            return Err(bad("truncated"));
        }
        moved.insert(at, u32::try_from(pos).map_err(|_| bad("too large"))?);
        pos += 30 + nn.len() as u64 + rest;
    }
    let new_cd = pos;
    for (r, local, name) in &ents {
        let rec = &cd[r.clone()];
        let nn = rename(name)?;
        out.write_all(&rec[..28])?;
        out.write_all(&(nn.len() as u16).to_le_bytes())?;
        out.write_all(&rec[30..42])?;
        out.write_all(&moved[local].to_le_bytes())?;
        out.write_all(&nn)?;
        out.write_all(&rec[46 + name.len()..])?;
        pos += (rec.len() - name.len() + nn.len()) as u64;
    }
    let size = u32::try_from(pos - new_cd).map_err(|_| bad("too large"))?;
    let off = u32::try_from(new_cd).map_err(|_| bad("too large"))?;
    out.write_all(&eocd[..12])?;
    out.write_all(&size.to_le_bytes())?;
    out.write_all(&off.to_le_bytes())?;
    out.write_all(&eocd[20..])?;
    Ok(pos + eocd.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A stored-method zip like ditto's layout: folders, files with data descriptors, __MACOSX.
    fn zip(names: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut cd) = (Vec::new(), Vec::new());
        for (name, data) in names {
            let off = out.len() as u32;
            let crc = 0x1234_5678u32; // not checked by the rename
            out.extend(LOC.to_le_bytes());
            out.extend([20, 0, 8, 0, 0, 0, 0, 0, 0, 0]); // version, flags (descriptor), method, time, date
            out.extend([0u8; 12]); // crc, sizes in the descriptor
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(4u16.to_le_bytes());
            out.extend(name.as_bytes());
            out.extend([0xaa; 4]); // extra
            out.extend(*data);
            out.extend(0x0807_4b50u32.to_le_bytes());
            out.extend(crc.to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            cd.extend(CEN.to_le_bytes());
            cd.extend([20, 3, 20, 0, 8, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend(crc.to_le_bytes());
            cd.extend((data.len() as u32).to_le_bytes());
            cd.extend((data.len() as u32).to_le_bytes());
            cd.extend((name.len() as u16).to_le_bytes());
            cd.extend(4u16.to_le_bytes());
            cd.extend(2u16.to_le_bytes()); // comment
            cd.extend([0u8; 4]); // disk, internal attributes
            cd.extend(0o100755u32.wrapping_shl(16).to_le_bytes());
            cd.extend(off.to_le_bytes());
            cd.extend(name.as_bytes());
            cd.extend([0xbb; 4]);
            cd.extend(b"c!");
        }
        let cd_off = out.len() as u32;
        out.extend(&cd);
        out.extend(EOCD.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend((names.len() as u16).to_le_bytes());
        out.extend((names.len() as u16).to_le_bytes());
        out.extend((cd.len() as u32).to_le_bytes());
        out.extend(cd_off.to_le_bytes());
        out.extend(3u16.to_le_bytes());
        out.extend(b"zzz");
        out
    }

    /// Minimal reader: (name, data, extra, comment, mode) per central entry, via its local header.
    fn read(z: &[u8]) -> Vec<(String, Vec<u8>, Vec<u8>, Vec<u8>, u32)> {
        let e = (0..z.len() - 21).rev().find(|&i| u32le(z, i) == EOCD).unwrap();
        let (n, mut p) = (u16le(z, e + 10) as usize, u32le(z, e + 16) as usize);
        assert_eq!(p + u32le(z, e + 12) as usize, e);
        (0..n)
            .map(|_| {
                let (nl, xl, cl) = (u16le(z, p + 28) as usize, u16le(z, p + 30) as usize, u16le(z, p + 32) as usize);
                let (size, mode, lo) = (u32le(z, p + 20) as usize, u32le(z, p + 38), u32le(z, p + 42) as usize);
                let name = z[p + 46..p + 46 + nl].to_vec();
                let r = (String::from_utf8(name.clone()).unwrap(), {
                    assert_eq!(u32le(z, lo), LOC);
                    let (lnl, lxl) = (u16le(z, lo + 26) as usize, u16le(z, lo + 28) as usize);
                    assert_eq!(&z[lo + 30..lo + 30 + lnl], &name[..], "local and central names agree");
                    z[lo + 30 + lnl + lxl..lo + 30 + lnl + lxl + size].to_vec()
                }, z[p + 46 + nl..p + 46 + nl + xl].to_vec(), z[p + 46 + nl + xl..p + 46 + nl + xl + cl].to_vec(), mode);
                p += 46 + nl + xl + cl;
                r
            })
            .collect()
    }

    #[test]
    fn renames_the_app_folder_only() {
        let src = zip(&[
            ("TetherQS.app/", b""),
            ("TetherQS.app/Contents/MacOS/TetherQS", b"\xcf\xfa\xed\xfe binary"),
            ("TetherQS.app/Contents/Info.plist", b"<plist/>"),
            ("__MACOSX/._TetherQS.app", b"AD"),
            ("__MACOSX/TetherQS.app/Contents/._Info.plist", b"AD2"),
        ]);
        let new = "ACME GmbH-Support [0123456789abcdef].app";
        let mut out = Vec::new();
        let n = rename_app(&mut Cursor::new(&src), &mut out, new).unwrap();
        assert_eq!(n as usize, out.len());
        let (a, b) = (read(&src), read(&out));
        let names: Vec<&str> = b.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            names,
            [
                format!("{new}/"),
                format!("{new}/Contents/MacOS/TetherQS"),
                format!("{new}/Contents/Info.plist"),
                format!("__MACOSX/._{new}"),
                format!("__MACOSX/{new}/Contents/._Info.plist"),
            ]
        );
        for (x, y) in a.iter().zip(&b) {
            assert_eq!((&x.1, &x.2, &x.3, x.4), (&y.1, &y.2, &y.3, y.4), "data, extra, comment, mode unchanged");
        }
        assert!(out.ends_with(b"zzz"));
        // Renaming back gives the original bytes.
        let mut back = Vec::new();
        rename_app(&mut Cursor::new(&out), &mut back, "TetherQS.app").unwrap();
        assert_eq!(back, src);
    }

    /// The relay's transform of a real release zip, for the Mac test (docs/macos.md):
    /// `TQS_ZIP_IN=<zip> TQS_ZIP_OUT=<zip> TQS_APP='<name>.app' cargo test -- --ignored real_zip`
    #[test]
    #[ignore]
    fn real_zip() {
        let var = |k| std::env::var(k).unwrap();
        let mut out = std::fs::File::create(var("TQS_ZIP_OUT")).unwrap();
        rename_app(&mut std::fs::File::open(var("TQS_ZIP_IN")).unwrap(), &mut out, &var("TQS_APP")).unwrap();
    }

    #[test]
    fn refuses_odd_archives() {
        let mut out = Vec::new();
        assert!(rename_app(&mut Cursor::new(zip(&[("TetherQS.app/", b""), ("other/x", b"")])), &mut out, "A.app").is_err());
        assert!(rename_app(&mut Cursor::new(zip(&[("readme.txt", b"")])), &mut out, "A.app").is_err());
        assert!(rename_app(&mut Cursor::new(zip(&[("T.app/", b"")])), &mut out, "../A.app/").is_err());
        assert!(rename_app(&mut Cursor::new(b"not a zip".to_vec()), &mut out, "A.app").is_err());
        let mut z = zip(&[("T.app/x", b"data")]);
        let n = z.len();
        z[n - 13] ^= 0xff; // central directory offset
        assert!(rename_app(&mut Cursor::new(z), &mut out, "A.app").is_err());
    }
}
