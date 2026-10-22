//! Compositions: a named group of roles, each pointing at a build of "Mes Builds" by name.

use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::build::{Build, BuildStore, Issue, load_list, write_atomic};
use crate::catalog::Catalog;
use crate::share::{base64_decode, base64_encode};

const PREFIX: &str = "AOC1:";

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub role: String,
    /// Name of a build in "Mes Builds"; `None` while the role has no build yet.
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub player: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composition {
    pub name: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub members: Vec<Member>,
}

/// A problem found while checking a composition against the available builds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CompIssue {
    EmptyName,
    /// Member `index` references a build that does not exist.
    MissingBuild {
        index: usize,
        role: String,
        build: String,
    },
}

impl fmt::Display for CompIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompIssue::EmptyName => write!(f, "la composition n'a pas de nom"),
            CompIssue::MissingBuild { role, build, .. } => {
                write!(f, "{role} : le build « {build} » n'existe plus")
            }
        }
    }
}

impl Composition {
    #[allow(dead_code)]
    pub fn resolve<'a>(&self, index: usize, builds: &'a [Build]) -> Option<&'a Build> {
        let name = self.members.get(index)?.build.as_deref()?;
        builds.iter().find(|b| b.name == name)
    }

    pub fn check(&self, builds: &[Build]) -> Vec<CompIssue> {
        let mut issues = Vec::new();
        if self.name.trim().is_empty() {
            issues.push(CompIssue::EmptyName);
        }
        for (index, m) in self.members.iter().enumerate() {
            if let Some(name) = &m.build
                && !builds.iter().any(|b| &b.name == name)
            {
                issues.push(CompIssue::MissingBuild {
                    index,
                    role: m.role.clone(),
                    build: name.clone(),
                });
            }
        }
        issues
    }

    /// Points members using build `old` at `new`; returns how many members changed.
    pub fn rename_build(&mut self, old: &str, new: &str) -> usize {
        let mut n = 0;
        for m in self
            .members
            .iter_mut()
            .filter(|m| m.build.as_deref() == Some(old))
        {
            m.build = Some(new.to_owned());
            n += 1;
        }
        n
    }

    /// Clears the reference of members using build `name` (after it was deleted); returns the count.
    pub fn unlink_build(&mut self, name: &str) -> usize {
        let mut n = 0;
        for m in self
            .members
            .iter_mut()
            .filter(|m| m.build.as_deref() == Some(name))
        {
            m.build = None;
            n += 1;
        }
        n
    }

    /// Distinct build names referenced by this composition, in member order.
    pub fn build_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for name in self.members.iter().filter_map(|m| m.build.as_deref()) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }
}

pub struct CompStore {
    path: PathBuf,
    pub comps: Vec<Composition>,
}

impl CompStore {
    /// Loads the store, falling back to the `.bak` copy if the main file is missing or unreadable.
    pub fn load(path: PathBuf) -> Self {
        let comps = load_list(&path);
        Self { path, comps }
    }

    /// Atomic write to `comps.json`, keeping the previous version as `comps.json.bak`.
    pub fn save(&self) -> Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(&self.comps)?, true)
    }

    /// Call after renaming a build; returns the number of members updated across all compositions.
    pub fn rename_build(&mut self, old: &str, new: &str) -> usize {
        self.comps
            .iter_mut()
            .map(|c| c.rename_build(old, new))
            .sum()
    }

    /// Call after deleting a build; returns the number of members left without a build.
    pub fn unlink_build(&mut self, name: &str) -> usize {
        self.comps.iter_mut().map(|c| c.unlink_build(name)).sum()
    }

    /// `(composition name, issues)` for every composition with a problem. Read-only.
    pub fn check_all(&self, builds: &[Build]) -> Vec<(String, Vec<CompIssue>)> {
        self.comps
            .iter()
            .filter_map(|c| {
                let issues = c.check(builds);
                (!issues.is_empty()).then(|| (c.name.clone(), issues))
            })
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
struct Bundle {
    comp: Composition,
    builds: Vec<Build>,
}

/// Shareable string (`AOC1:` + URL-safe base64 of compact JSON) holding the composition and the
/// builds it references; builds that no longer exist are left out (see [`Composition::check`]).
pub fn encode(comp: &Composition, builds: &[Build]) -> String {
    let bundle = Bundle {
        comp: comp.clone(),
        builds: comp
            .build_names()
            .into_iter()
            .filter_map(|n| builds.iter().find(|b| b.name == n).cloned())
            .collect(),
    };
    let json = serde_json::to_vec(&bundle).expect("a composition always serializes");
    format!("{PREFIX}{}", base64_encode(&json))
}

/// A decoded composition share, with its builds already repaired against the catalog.
pub struct CompImport {
    pub comp: Composition,
    pub builds: Vec<Build>,
    /// `(build name, issues)` for builds that needed repairs.
    pub issues: Vec<(String, Vec<Issue>)>,
}

pub fn import(text: &str, catalog: &Catalog) -> Result<CompImport> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(payload) = compact.strip_prefix(PREFIX) else {
        bail!("ce texte n'est pas une composition partagée (préfixe {PREFIX} manquant)");
    };
    let bytes = base64_decode(payload).context("code de composition corrompu")?;
    let Bundle {
        mut comp,
        mut builds,
    } = serde_json::from_slice(&bytes).context("contenu de la composition illisible")?;
    comp.name = comp.name.trim().to_owned();
    let mut issues = Vec::new();
    for b in &mut builds {
        b.name = b.name.trim().to_owned();
        let found = b.repair(catalog);
        if !found.is_empty() {
            issues.push((b.name.clone(), found));
        }
    }
    Ok(CompImport {
        comp,
        builds,
        issues,
    })
}

impl CompImport {
    /// Adds the imported builds to `store` and returns the composition, ready to push onto `CompStore::comps`.
    /// A build identical to an existing one of the same name is reused; a different one is added
    /// under a free name ("X (importé)", "X (importé 2)", …) and the composition is pointed at it.
    pub fn apply(self, store: &mut BuildStore) -> Composition {
        let CompImport {
            mut comp, builds, ..
        } = self;
        for build in builds {
            let same = |a: &Build, b: &Build| {
                serde_json::to_string(a).ok() == serde_json::to_string(b).ok()
            };
            match store.builds.iter().find(|b| b.name == build.name) {
                None => store.builds.push(build),
                Some(existing) if same(existing, &build) => {}
                Some(_) => {
                    let old = build.name.clone();
                    let new = (1..)
                        .map(|n| {
                            if n == 1 {
                                format!("{old} (importé)")
                            } else {
                                format!("{old} (importé {n})")
                            }
                        })
                        .find(|candidate| !store.builds.iter().any(|b| &b.name == candidate))
                        .expect("unbounded range");
                    comp.rename_build(&old, &new);
                    store.builds.push(Build { name: new, ..build });
                }
            }
        }
        comp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::build::{Equipped, sibling};
    use crate::catalog::Slot;

    fn catalog() -> Catalog {
        Catalog::load(Path::new("assets/data")).expect("catalog")
    }

    fn build(name: &str, item: &str) -> Build {
        let mut b = Build {
            name: name.into(),
            ..Default::default()
        };
        b.set(
            Slot::MainHand,
            Equipped {
                item: item.into(),
                ..Default::default()
            },
        );
        b
    }

    fn member(role: &str, build: Option<&str>) -> Member {
        Member {
            role: role.into(),
            build: build.map(Into::into),
            ..Default::default()
        }
    }

    fn sample() -> Composition {
        Composition {
            name: "ZvZ".into(),
            notes: "Rendez-vous 20h".into(),
            members: vec![
                member("Tank", Some("Tank A")),
                member("Heal", Some("Heal A")),
                member("Scout", None),
                member("Off-tank", Some("Tank A")),
            ],
        }
    }

    #[test]
    fn resolve_and_check() {
        let builds = vec![build("Tank A", "T8_MAIN_SWORD")];
        let comp = sample();
        assert_eq!(
            comp.resolve(0, &builds).map(|b| b.name.as_str()),
            Some("Tank A")
        );
        assert!(
            comp.resolve(1, &builds).is_none()
                && comp.resolve(2, &builds).is_none()
                && comp.resolve(9, &builds).is_none()
        );
        assert_eq!(
            comp.check(&builds),
            vec![CompIssue::MissingBuild {
                index: 1,
                role: "Heal".into(),
                build: "Heal A".into()
            }]
        );
        let blank = Composition {
            name: " ".into(),
            ..Default::default()
        };
        assert_eq!(blank.check(&builds), vec![CompIssue::EmptyName]);
    }

    #[test]
    fn rename_and_unlink_propagate() {
        let mut store = CompStore {
            path: PathBuf::new(),
            comps: vec![sample(), sample()],
        };
        assert_eq!(store.rename_build("Tank A", "Tank B"), 4);
        assert_eq!(store.comps[0].members[3].build.as_deref(), Some("Tank B"));
        assert_eq!(store.unlink_build("Tank B"), 4);
        assert_eq!(store.comps[1].members[0].build, None);
        assert_eq!(
            store.rename_build("nope", "x") + store.unlink_build("nope"),
            0
        );
    }

    #[test]
    fn store_round_trips_with_backup() {
        let dir = std::env::temp_dir().join(format!("aocb-comps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("comps.json");

        let mut store = CompStore::load(path.clone());
        store.comps.push(sample());
        store.save().unwrap();
        store.comps[0].notes = "v2".into();
        store.save().unwrap();
        assert_eq!(CompStore::load(path.clone()).comps[0].notes, "v2");
        assert!(sibling(&path, "bak").exists() && !sibling(&path, "tmp").exists());

        std::fs::write(&path, b"{ truncated").unwrap();
        assert_eq!(
            CompStore::load(path.clone()).comps[0].notes,
            "Rendez-vous 20h",
            "falls back to .bak"
        );
        assert!(sibling(&path, "corrupt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_files_missing_optional_fields_load() {
        let comp: Composition =
            serde_json::from_str(r#"{"name":"A","members":[{"role":"Tank"}]}"#).unwrap();
        assert_eq!(comp.members[0], member("Tank", None));
    }

    #[test]
    fn share_round_trips_with_referenced_builds_only() {
        let c = catalog();
        let builds = vec![
            build("Tank A", "T8_MAIN_SWORD"),
            build("Heal A", "T8_MAIN_NATURESTAFF"),
            build("Unused", "T8_MAIN_SWORD"),
        ];
        let code = encode(&sample(), &builds);
        let imported = import(&format!(" {}\n{} ", &code[..30], &code[30..]), &c).unwrap();
        assert_eq!(imported.comp, sample());
        let names: Vec<_> = imported.builds.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Tank A", "Heal A"]);
        assert!(import("hello", &c).is_err() && import("AOC1:!!", &c).is_err());
    }

    #[test]
    fn import_repairs_builds() {
        let c = catalog();
        let code = encode(
            &sample(),
            &[build("Tank A", "NOPE"), build("Heal A", "NOPE")],
        );
        let imported = import(&code, &c).unwrap();
        assert_eq!(imported.issues.len(), 2);
        assert!(imported.builds.iter().all(|b| b.slots.is_empty()));
    }

    #[test]
    fn apply_reuses_identical_and_renames_conflicts() {
        let c = catalog();
        let theirs = vec![
            build("Tank A", "T8_MAIN_SWORD"),
            build("Heal A", "T8_MAIN_NATURESTAFF"),
        ];
        let code = encode(&sample(), &theirs);

        // Only the item ids matter here; repair would drop unknown ones, so use real catalog items.
        let mut store = BuildStore::load(PathBuf::from("does-not-exist.json"));
        let first = import(&code, &c).unwrap().apply(&mut store);
        assert_eq!(first, sample());
        let count = store.builds.len();
        // Same import again: identical builds are reused, nothing is duplicated.
        import(&code, &c).unwrap().apply(&mut store);
        assert_eq!(store.builds.len(), count);

        // A local build with the same name but different content forces a rename.
        store
            .builds
            .iter_mut()
            .find(|b| b.name == "Tank A")
            .unwrap()
            .get_mut(Slot::MainHand)
            .unwrap()
            .enchant = 2;
        let comp = import(&code, &c).unwrap().apply(&mut store);
        assert_eq!(comp.members[0].build.as_deref(), Some("Tank A (importé)"));
        assert_eq!(comp.members[3].build.as_deref(), Some("Tank A (importé)"));
        assert!(store.builds.iter().any(|b| b.name == "Tank A (importé)"));
        assert_eq!(comp.check(&store.builds), vec![]);
    }

    #[test]
    fn share_carries_build_note_and_swaps() {
        let c = catalog();
        let mut tank = build("Tank A", "T8_MAIN_SWORD");
        tank.note = "Engage en premier".into();
        tank.get_mut(Slot::MainHand)
            .unwrap()
            .add_swap(crate::build::Swap {
                item: "T8_MAIN_SWORD".into(),
                note: "alt".into(),
                ..Default::default()
            });
        let imported = import(&encode(&sample(), &[tank]), &c).unwrap();
        let b = &imported.builds[0];
        assert_eq!(b.note, "Engage en premier");
        assert_eq!(b.get(Slot::MainHand).unwrap().swaps[0].note, "alt");
    }
}
