//! Sharing builds: a copy-pasteable string (`AOB1:` + URL-safe base64 of compact JSON) and `.json` files.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::build::{Build, Issue, write_atomic};
use crate::catalog::Catalog;

const PREFIX: &str = "AOB1:";
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Shareable string for a build.
pub fn encode(build: &Build) -> String {
    let json = serde_json::to_vec(build).expect("a build always serializes");
    format!("{PREFIX}{}", base64_encode(&json))
}

/// Parses a share string. Surrounding whitespace and line breaks (from chat clients) are ignored.
pub fn decode(text: &str) -> Result<Build> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(payload) = compact.strip_prefix(PREFIX) else {
        bail!("ce texte n'est pas un build partagé (préfixe {PREFIX} manquant)");
    };
    let bytes = base64_decode(payload).context("code de build corrompu")?;
    serde_json::from_slice(&bytes).context("contenu du build illisible")
}

/// Imports from a share string or raw build JSON, then repairs the result against the catalog.
/// The issues list is what had to be fixed or dropped; show it to the user.
pub fn import(text: &str, catalog: &Catalog) -> Result<(Build, Vec<Issue>)> {
    let trimmed = text.trim();
    let mut build = if trimmed.starts_with('{') {
        serde_json::from_str(trimmed).context("JSON de build illisible")?
    } else {
        decode(trimmed)?
    };
    let issues = repair_import(&mut build, catalog);
    Ok((build, issues))
}

/// Writes the build as pretty JSON.
pub fn export_file(build: &Build, path: &Path) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(build)?, false)
}

/// Reads a `.json` build file (or one containing a share string) and repairs it.
pub fn import_file(path: &Path, catalog: &Catalog) -> Result<(Build, Vec<Issue>)> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("lecture de {}", path.display()))?;
    import(&text, catalog)
}

fn repair_import(build: &mut Build, catalog: &Catalog) -> Vec<Issue> {
    build.name = build.name.trim().to_owned();
    build.repair(catalog)
}

pub(crate) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

pub(crate) fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let v = match c {
            b'+' => 62,
            b'/' => 63,
            _ => ALPHABET
                .iter()
                .position(|&a| a == c)
                .with_context(|| format!("caractère invalide « {} »", c as char))?
                as u32,
        };
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Equipped;
    use crate::catalog::Slot;

    fn catalog() -> Catalog {
        Catalog::load(Path::new("assets/data")).expect("catalog")
    }

    fn sample() -> Build {
        let mut b = Build {
            name: "Frappeur".into(),
            ..Default::default()
        };
        b.set(
            Slot::MainHand,
            Equipped {
                item: "T8_MAIN_SWORD".into(),
                enchant: 1,
                spells: vec![None, None, None],
                passives: vec![None],
                ..Default::default()
            },
        );
        b
    }

    #[test]
    fn base64_round_trips_all_lengths() {
        for len in 0..40usize {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(
                base64_decode(&base64_encode(&data)).unwrap(),
                data,
                "len {len}"
            );
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE");
    }

    #[test]
    fn share_string_round_trips() {
        let b = sample();
        let code = encode(&b);
        assert!(code.starts_with(PREFIX) && !code.contains(char::is_whitespace));
        let back = decode(&format!("  {}\n{}  ", &code[..20], &code[20..])).unwrap();
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(decode("hello").is_err());
        assert!(decode("AOB1:!!!").is_err());
        assert!(decode("AOB1:TWFu").is_err());
    }

    #[test]
    fn import_repairs_against_catalog() {
        let c = catalog();
        let mut b = sample();
        b.name = "  Frappeur ".into();
        b.get_mut(Slot::MainHand).unwrap().enchant = 7;
        b.set(
            Slot::Head,
            Equipped {
                item: "NOPE".into(),
                ..Default::default()
            },
        );
        let (back, issues) = import(&encode(&b), &c).unwrap();
        assert_eq!(back.name, "Frappeur");
        assert_eq!(back.get(Slot::MainHand).unwrap().enchant, 4);
        assert!(back.get(Slot::Head).is_none());
        assert!(issues.len() >= 2, "{issues:?}");
        // Raw JSON is accepted too.
        assert!(import(&serde_json::to_string(&b).unwrap(), &c).is_ok());
    }

    #[test]
    fn file_round_trips() {
        let c = catalog();
        let path = std::env::temp_dir().join(format!("aocb-share-{}.json", std::process::id()));
        let (b, _) = import(&encode(&sample()), &c).unwrap();
        export_file(&b, &path).unwrap();
        let (back, issues) = import_file(&path, &c).unwrap();
        assert_eq!(back.name, "Frappeur");
        assert!(issues.is_empty(), "{issues:?}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn note_and_swaps_travel_in_share_string() {
        let c = catalog();
        let mut b = sample();
        b.note = "Ouvrir avec Q".into();
        b.get_mut(Slot::MainHand)
            .unwrap()
            .add_swap(crate::build::Swap {
                item: "T8_MAIN_SWORD".into(),
                enchant: 2,
                spells: vec![None; 3],
                passives: vec![None],
                note: "contre les mages".into(),
            });
        let back = decode(&encode(&b)).unwrap();
        assert_eq!(back.note, "Ouvrir avec Q");
        let swap = &back.get(Slot::MainHand).unwrap().swaps[0];
        assert_eq!((swap.enchant, swap.note.as_str()), (2, "contre les mages"));
        let (imported, _) = import(&encode(&b), &c).unwrap();
        assert_eq!(imported.get(Slot::MainHand).unwrap().swaps.len(), 1);
    }

    #[test]
    fn old_share_strings_without_note_or_swaps_decode() {
        let old = r#"{"name":"Vieux","slots":{"0":{"item":"T8_MAIN_SWORD","enchant":0,"spells":[],"passives":[]}}}"#;
        let code = format!("AOB1:{}", base64_encode(old.as_bytes()));
        let b = decode(&code).unwrap();
        assert_eq!(b.note, "");
        assert!(b.get(Slot::MainHand).unwrap().swaps.is_empty());
    }
}
