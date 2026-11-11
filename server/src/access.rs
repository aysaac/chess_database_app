//! Who may do what with a database. Every route that touches a database's
//! games, positions, searches or imports goes through [`require`].
//!
//! - The owner: everything.
//! - Private: nobody else, the admin included. Shares are kept but give
//!   nothing while the database is private.
//! - Shared: each share gives read, or read and import (write).
//! - Server-wide: every user reads; shares can still give write.
//!
//! Being an admin gives no access to games: the admin routes
//! (`routes/admin.rs`) return metadata only.

use crate::error::{ApiError, ApiResult};
use crate::secret::is_db_id;
use crate::store::{DbMeta, Store, User};
use crate::wire::{Access, ShareAccess, Visibility};

pub fn access(user: &User, db: &DbMeta, share: Option<ShareAccess>) -> Option<Access> {
    if user.id == db.owner_id {
        return Some(Access::Owner);
    }
    match (db.visibility, share) {
        (Visibility::Private, _) => None,
        (_, Some(ShareAccess::Write)) => Some(Access::Write),
        (_, Some(ShareAccess::Read)) => Some(Access::Read),
        (Visibility::Server, None) => Some(Access::Read),
        (Visibility::Shared, None) => None,
    }
}

/// The database `id` if `user` may do `need` with it. One they can't read
/// is "not found", the same as one that doesn't exist (and so is an id
/// that isn't one: no path is ever built from it); one they can read but
/// not change is "forbidden".
pub fn require(store: &Store, user: &User, id: &str, need: Access) -> ApiResult<(DbMeta, Access)> {
    if !is_db_id(id) {
        return Err(ApiError::not_found());
    }
    let meta = store.database(id)?.ok_or_else(ApiError::not_found)?;
    let share = if user.id == meta.owner_id { None } else { store.share(id, user.id)? };
    let have = access(user, &meta, share).ok_or_else(ApiError::not_found)?;
    if have < need {
        return Err(ApiError::forbidden(match need {
            Access::Owner => "Only the database's owner can do that",
            _ => "You can read this database but not import into it",
        }));
    }
    Ok((meta, have))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Role;

    fn user(id: i64, role: Role) -> User {
        User { id, username: format!("u{id}"), role, disabled: false, quota_bytes: None, created_at: 0 }
    }

    fn db(owner: i64, visibility: Visibility) -> DbMeta {
        DbMeta {
            id: "x".into(),
            name: "x".into(),
            owner_id: owner,
            owner: "o".into(),
            visibility,
            quota_bytes: None,
            created_at: 0,
        }
    }

    #[test]
    fn rules() {
        let (owner, other, admin) = (user(1, Role::User), user(2, Role::User), user(3, Role::Admin));
        use ShareAccess::{Read, Write};
        use Visibility::{Private, Server, Shared};
        assert_eq!(access(&owner, &db(1, Private), None), Some(Access::Owner));
        for share in [None, Some(Read), Some(Write)] {
            assert_eq!(access(&other, &db(1, Private), share), None);
            assert_eq!(access(&admin, &db(1, Private), share), None);
        }
        assert_eq!(access(&other, &db(1, Shared), None), None);
        assert_eq!(access(&admin, &db(1, Shared), None), None);
        assert_eq!(access(&other, &db(1, Shared), Some(Read)), Some(Access::Read));
        assert_eq!(access(&other, &db(1, Shared), Some(Write)), Some(Access::Write));
        assert_eq!(access(&other, &db(1, Server), None), Some(Access::Read));
        assert_eq!(access(&admin, &db(1, Server), None), Some(Access::Read));
        assert_eq!(access(&other, &db(1, Server), Some(Write)), Some(Access::Write));
    }
}
