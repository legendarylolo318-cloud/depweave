//! Sync database reader (`*.db` = tar + gzip/zstd). Only package names are needed.

use crate::error::{io_err, Error, Result};
use std::io::Read;
use std::path::Path;

/// Strip `-pkgver-pkgrel` from a sync-db directory entry like `bash-5.2.26-2`.
pub fn name_from_entry(dir: &str) -> Option<&str> {
    let mut it = dir.rsplitn(3, '-');
    let _rel = it.next()?;
    let _ver = it.next()?;
    it.next().filter(|n| !n.is_empty())
}

/// Return the package names contained in a sync db file.
pub fn read_sync_names(path: &Path) -> Result<Vec<String>> {
    let data = std::fs::read(path).map_err(|e| io_err(path, e))?;
    names_from_bytes(&data).map_err(|e| match e {
        Error::Parse(s) => Error::Parse(format!("{}: {s}", path.display())),
        Error::Unsupported(s) => Error::Unsupported(format!("{}: {s}", path.display())),
        o => o,
    })
}

pub fn names_from_bytes(data: &[u8]) -> Result<Vec<String>> {
    if data.starts_with(&[0x1f, 0x8b]) {
        tar_names(flate2::read::GzDecoder::new(data))
    } else if data.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        let dec =
            ruzstd::StreamingDecoder::new(data).map_err(|e| Error::Parse(format!("zstd: {e}")))?;
        tar_names(dec)
    } else if data.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        // custom repos built with `repo-add foo.db.tar.xz`
        let mut out = Vec::new();
        lzma_rs::xz_decompress(&mut std::io::BufReader::new(data), &mut out)
            .map_err(|e| Error::Parse(format!("xz: {e}")))?;
        tar_names(&out[..])
    } else if data.starts_with(b"BZh") {
        Err(Error::Unsupported("bzip2-compressed sync db".into()))
    } else if data.len() > 262 && &data[257..262] == b"ustar" {
        tar_names(data)
    } else {
        Err(Error::Parse("unknown sync db format".into()))
    }
}

fn tar_names<R: Read>(r: R) -> Result<Vec<String>> {
    let mut ar = tar::Archive::new(r);
    let mut out = Vec::new();
    let entries = ar
        .entries()
        .map_err(|e| Error::Parse(format!("tar: {e}")))?;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log::warn!("truncated/corrupt sync db entry: {e}");
                break;
            }
        };
        let Ok(p) = entry.path() else { continue };
        let p = p.to_string_lossy().into_owned();
        if let Some(dir) = p.strip_suffix("/desc") {
            match name_from_entry(dir) {
                Some(n) => out.push(n.to_string()),
                None => log::warn!("odd sync db entry: {p}"),
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn make_tar_gz(entries: &[&str]) -> Vec<u8> {
    use std::io::Write;
    let mut tarb = tar::Builder::new(Vec::new());
    for e in entries {
        let body = b"%NAME%\nx\n";
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tarb.append_data(&mut h, format!("{e}/desc"), &body[..])
            .unwrap();
    }
    let raw = tarb.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw).unwrap();
    gz.finish().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names() {
        assert_eq!(name_from_entry("bash-5.2.26-2"), Some("bash"));
        assert_eq!(name_from_entry("lib32-gtk-3-3.24.1-1"), Some("lib32-gtk-3"));
        assert_eq!(name_from_entry("nodash"), None);
    }

    #[test]
    fn reads_gzip_db() {
        let db = make_tar_gz(&["bash-5.2-1", "gtk-3-3.24-1"]);
        let mut n = names_from_bytes(&db).unwrap();
        n.sort();
        assert_eq!(n, vec!["bash", "gtk-3"]);
    }

    #[test]
    fn rejects_garbage_and_corrupt_xz() {
        assert!(matches!(names_from_bytes(b"hello"), Err(Error::Parse(_))));
        assert!(matches!(
            names_from_bytes(&[0xfd, b'7', b'z', b'X', b'Z', 0, 1]),
            Err(Error::Parse(_))
        ));
        assert!(matches!(
            names_from_bytes(b"BZh91AY&SY"),
            Err(Error::Unsupported(_))
        ));
        assert!(names_from_bytes(&[]).is_err());
    }

    fn raw_tar(entries: &[&str]) -> Vec<u8> {
        let mut tarb = tar::Builder::new(Vec::new());
        for e in entries {
            let body = b"%NAME%\nx\n";
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tarb.append_data(&mut h, format!("{e}/desc"), &body[..])
                .unwrap();
        }
        tarb.into_inner().unwrap()
    }

    #[test]
    fn reads_plain_tar_xz_and_zstd_dbs() {
        let raw = raw_tar(&["foo-1-1", "bar-baz-2:3.0-4"]);
        let mut n = names_from_bytes(&raw).unwrap();
        n.sort();
        assert_eq!(n, vec!["bar-baz", "foo"]);

        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut &raw[..], &mut xz).unwrap();
        let mut n = names_from_bytes(&xz).unwrap();
        n.sort();
        assert_eq!(n, vec!["bar-baz", "foo"]);

        // fixture produced with `zstd -19` from a tar holding core-like entries
        let zst = include_bytes!("../tests/fixtures/sync.db.tar.zst");
        let mut n = names_from_bytes(zst).unwrap();
        n.sort();
        assert_eq!(n, vec!["bash", "lib32-gtk-3", "zlib"]);
    }

    #[test]
    fn truncated_db_keeps_complete_entries() {
        let db = make_tar_gz(&["a-1-1", "b-1-1", "c-1-1"]);
        // a cut-off download must not panic
        let r = names_from_bytes(&db[..db.len() / 2]);
        assert!(r.is_ok() || r.is_err());
        let raw = raw_tar(&["a-1-1", "b-1-1", "c-1-1"]);
        let n = names_from_bytes(&raw[..1536]).unwrap();
        assert_eq!(n, vec!["a", "b"]);
    }
}
