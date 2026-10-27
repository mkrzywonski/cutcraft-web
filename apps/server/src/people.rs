//! Who the design work is for, and their private area (spec §6).
//!
//! Every request that touches a design session belongs to a principal: the signed-in person (by
//! their account's stable id, so their area survives a change of name or password), or, in open
//! mode, the household. Each principal has a private area, readable by no one else:
//! `data/people/<id>/` with their designs, fonts, drafts and settings.

use std::path::{Path, PathBuf};

use crate::users::Identity;

/// The principal while no accounts exist (open mode). When the first admin is created, the
/// household's area becomes theirs ([`adopt_household`]).
pub const HOUSEHOLD: &str = "household";

/// The folders in each area.
const FOLDERS: &[&str] = &["designs", "fonts", "drafts"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    /// The account id, or [`HOUSEHOLD`].
    pub id: String,
    /// For messages and audit only; never for finding files.
    pub name: String,
}

impl Principal {
    /// The principal a request acts as, if any.
    pub fn of(identity: &Identity) -> Option<Self> {
        match identity {
            Identity::Open => Some(Self {
                id: HOUSEHOLD.into(),
                name: "the household".into(),
            }),
            Identity::User {
                id,
                name,
                must_change: false,
                ..
            } if valid_id(id) => Some(Self {
                id: id.clone(),
                name: name.clone(),
            }),
            _ => None,
        }
    }
}

/// An id is lowercase hex, or the household: never a path.
fn valid_id(id: &str) -> bool {
    id == HOUSEHOLD
        || (!id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A principal's private area, made if it doesn't exist, owner-only (0700).
pub fn area(people: &Path, principal: &Principal) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if !valid_id(&principal.id) {
        return Err(std::io::Error::other("not a principal id"));
    }
    let dir = people.join(&principal.id);
    for folder in FOLDERS {
        std::fs::create_dir_all(dir.join(folder))?;
    }
    for d in [people.to_path_buf(), dir.clone()] {
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// The first admin takes over what the household made in open mode (spec §6). Does nothing if the
/// household has no area, or the admin already has one.
pub fn adopt_household(people: &Path, id: &str) -> std::io::Result<()> {
    if !valid_id(id) || id == HOUSEHOLD {
        return Err(std::io::Error::other("not an account id"));
    }
    let (from, to) = (people.join(HOUSEHOLD), people.join(id));
    if from.is_dir() && !to.exists() {
        std::fs::rename(from, to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::Role;

    fn person(id: &str) -> Identity {
        Identity::User {
            name: "Sam".into(),
            id: id.into(),
            role: Role::User,
            must_change: false,
        }
    }

    #[test]
    fn principals_come_from_the_identity_and_never_from_a_bad_id() {
        assert_eq!(Principal::of(&Identity::Open).unwrap().id, HOUSEHOLD);
        assert_eq!(Principal::of(&person("a1b2c3")).unwrap().id, "a1b2c3");
        assert!(Principal::of(&person("../etc")).is_none());
        assert!(Principal::of(&person("")).is_none());
        assert!(Principal::of(&Identity::Nobody).is_none());
        assert!(Principal::of(&Identity::Locked).is_none());
    }

    #[test]
    fn areas_are_private_and_separate() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let people = dir.path().join("people");
        let a = area(&people, &Principal::of(&person("aa11")).unwrap()).unwrap();
        let b = area(&people, &Principal::of(&person("bb22")).unwrap()).unwrap();
        assert_ne!(a, b);
        for d in [&people, &a] {
            assert_eq!(
                std::fs::metadata(d).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert!(
            a.join("designs").is_dir() && a.join("drafts").is_dir() && a.join("fonts").is_dir()
        );
    }

    #[test]
    fn the_first_admin_adopts_the_households_area() {
        let dir = tempfile::tempdir().unwrap();
        let people = dir.path().join("people");
        let household = area(&people, &Principal::of(&Identity::Open).unwrap()).unwrap();
        std::fs::write(household.join("designs").join("d.svg"), b"<svg/>").unwrap();
        adopt_household(&people, "cc33").unwrap();
        assert!(people.join("cc33/designs/d.svg").is_file());
        assert!(!people.join(HOUSEHOLD).exists());
        assert!(adopt_household(&people, "../x").is_err());
    }
}
