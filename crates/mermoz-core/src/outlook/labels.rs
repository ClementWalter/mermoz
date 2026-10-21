//! Outlook folders and categories seen as Gmail-style labels, so the store, the issue
//! model and the write-back queue work unchanged.
//!
//! - Well-known folders are system labels: Inbox `INBOX`, Sent Items `SENT`, Deleted
//!   Items `TRASH`, Junk Email `SPAM`, Drafts `DRAFT`; Archive is "no label" (archived).
//! - Other folders are user labels `folder:<id>` named by their path ("Clients/Acme"),
//!   so mail filed in folders shows under projects of those names.
//! - Categories are user labels `cat:<name>`. Mermoz's projects and statuses
//!   (`Mermoz/To do`…) are written as categories: like labels, a message carries any
//!   number of them, while it sits in exactly one folder.
//! - `UNREAD` is `isRead: false`; `STARRED` is a flagged message.
//!
//! Write-back turns label changes back into Outlook moves and patches ([`plan_thread`]).

use super::{Folder, MessageMeta};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

/// Prefix of a user folder's label id.
pub const FOLDER: &str = "folder:";
/// Prefix of a category's label id.
pub const CATEGORY: &str = "cat:";

/// Where a folder sits in the mailbox, as far as labels go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Inbox,
    Sent,
    Trash,
    Spam,
    Drafts,
    Archive,
    /// A folder the user made: its label id and name.
    Folder {
        label: String,
        name: String,
    },
    /// Outbox, sync logs and the like: never imported.
    Skip,
}

/// Graph's well-known folder names and the place each one is.
pub const WELL_KNOWN: &[(&str, Place)] = &[
    ("inbox", Place::Inbox),
    ("sentitems", Place::Sent),
    ("deleteditems", Place::Trash),
    ("junkemail", Place::Spam),
    ("drafts", Place::Drafts),
    ("archive", Place::Archive),
    ("outbox", Place::Skip),
    ("conversationhistory", Place::Skip),
    ("syncissues", Place::Skip),
    ("scheduled", Place::Skip),
];

/// The mailbox's folders by id, with their places.
#[derive(Debug, Clone, Default)]
pub struct FolderMap {
    places: HashMap<String, Place>,
    /// Folder id of each well-known name found.
    roles: HashMap<String, String>,
    totals: HashMap<String, i64>,
}

impl FolderMap {
    /// `roles`: well-known name → folder id; `all`: every folder of the mailbox.
    pub fn build(roles: HashMap<String, String>, all: &[Folder]) -> Self {
        let by_id: HashMap<&str, &Folder> = all.iter().map(|f| (f.id.as_str(), f)).collect();
        let role_place: HashMap<&str, Place> = roles
            .iter()
            .filter_map(|(name, id)| {
                WELL_KNOWN
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, p)| (id.as_str(), p.clone()))
            })
            .collect();
        let mut places = HashMap::new();
        for f in all {
            let place = match role_place.get(f.id.as_str()) {
                Some(p) => p.clone(),
                None => place_of_user_folder(f, &by_id, &role_place),
            };
            places.insert(f.id.clone(), place);
        }
        // Well-known folders the listing missed (e.g. hidden ones) still have a place.
        for (id, p) in &role_place {
            places.entry(id.to_string()).or_insert_with(|| p.clone());
        }
        let totals = all
            .iter()
            .map(|f| (f.id.clone(), f.total_item_count))
            .collect();
        Self {
            places,
            roles,
            totals,
        }
    }

    pub fn place(&self, folder_id: &str) -> Option<&Place> {
        self.places.get(folder_id)
    }

    pub fn knows(&self, folder_id: &str) -> bool {
        self.places.contains_key(folder_id)
    }

    /// The id of a well-known folder (`inbox`, `archive`…).
    pub fn role(&self, name: &str) -> Option<&str> {
        self.roles.get(name).map(String::as_str)
    }

    /// Folders whose mail Mermoz imports: Inbox, Sent Items, Archive and the user's
    /// own folders. Inbox first, so new mail shows soonest.
    pub fn synced(&self) -> Vec<String> {
        let mut ids: Vec<(u8, String)> = self
            .places
            .iter()
            .filter_map(|(id, p)| {
                let rank = match p {
                    Place::Inbox => 0,
                    Place::Sent => 1,
                    Place::Archive => 2,
                    Place::Folder { .. } => 3,
                    _ => return None,
                };
                Some((rank, id.clone()))
            })
            .collect();
        ids.sort();
        ids.into_iter().map(|(_, id)| id).collect()
    }

    pub fn is_synced(&self, folder_id: &str) -> bool {
        matches!(
            self.place(folder_id),
            Some(Place::Inbox | Place::Sent | Place::Archive | Place::Folder { .. })
        )
    }

    /// How many messages the imported folders hold.
    pub fn total(&self) -> i64 {
        self.synced()
            .iter()
            .map(|id| self.totals.get(id).copied().unwrap_or(0))
            .sum()
    }

    /// The user folder of that name, by folder id.
    pub fn folder_named(&self, name: &str) -> Option<&str> {
        self.places.iter().find_map(|(id, p)| match p {
            Place::Folder { name: n, .. } if n == name => Some(id.as_str()),
            _ => None,
        })
    }

    /// (label id, name) of every user folder.
    pub fn folder_labels(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .places
            .values()
            .filter_map(|p| match p {
                Place::Folder { label, name } => Some((label.clone(), name.clone())),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// The folder a label id stands for (`folder:<id>` → `<id>`).
    pub fn folder_of_label(label: &str) -> Option<&str> {
        label.strip_prefix(FOLDER)
    }
}

/// A user folder's place: under Deleted Items it is trash, under Junk it is spam,
/// under Drafts or a skipped folder it is skipped; otherwise it is named by its path
/// below the well-known folders ("Inbox/Clients/Acme" is "Clients/Acme").
fn place_of_user_folder(
    f: &Folder,
    by_id: &HashMap<&str, &Folder>,
    roles: &HashMap<&str, Place>,
) -> Place {
    let mut path = vec![f.display_name.clone()];
    let mut parent = f.parent_folder_id.as_deref();
    // Bounded walk: a malformed listing with a cycle must not hang the sync.
    for _ in 0..64 {
        let Some(pid) = parent else { break };
        if let Some(p) = roles.get(pid) {
            match p {
                Place::Trash => return Place::Trash,
                Place::Spam => return Place::Spam,
                Place::Drafts | Place::Skip => return Place::Skip,
                _ => break,
            }
        }
        let Some(pf) = by_id.get(pid) else { break };
        path.push(pf.display_name.clone());
        parent = pf.parent_folder_id.as_deref();
    }
    path.reverse();
    Place::Folder {
        label: format!("{FOLDER}{}", f.id),
        name: path.join("/"),
    }
}

/// System label names Outlook keeps as folders, read state or flags, never categories.
const SYSTEM: &[&str] = &[
    "INBOX",
    "UNREAD",
    "SENT",
    "DRAFT",
    "SPAM",
    "TRASH",
    "STARRED",
    "IMPORTANT",
    "CHAT",
];

fn place_labels(place: &Place) -> Option<String> {
    match place {
        Place::Inbox => Some("INBOX".into()),
        Place::Sent => Some("SENT".into()),
        Place::Trash => Some("TRASH".into()),
        Place::Spam => Some("SPAM".into()),
        Place::Drafts => Some("DRAFT".into()),
        Place::Folder { label, .. } => Some(label.clone()),
        Place::Archive | Place::Skip => None,
    }
}

fn labels_of(
    place: Option<&Place>,
    categories: &BTreeSet<String>,
    unread: bool,
    starred: bool,
) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = categories
        .iter()
        .map(|c| format!("{CATEGORY}{c}"))
        .collect();
    out.extend(place.and_then(place_labels));
    if unread {
        out.insert("UNREAD".into());
    }
    if starred {
        out.insert("STARRED".into());
    }
    out
}

/// A message's labels from its folder, categories, read state and flag.
pub fn labels_for(m: &MessageMeta, folders: &FolderMap) -> BTreeSet<String> {
    let place = m.parent_folder_id.as_deref().and_then(|f| folders.place(f));
    labels_of(
        place,
        &m.categories.iter().cloned().collect(),
        m.is_read == Some(false),
        m.flagged(),
    )
}

/// Where a message is, from its stored labels.
fn place_from_labels(labels: &BTreeSet<String>) -> Place {
    let has = |l: &str| labels.contains(l);
    if has("TRASH") {
        Place::Trash
    } else if has("SPAM") {
        Place::Spam
    } else if has("DRAFT") {
        Place::Drafts
    } else if has("INBOX") {
        Place::Inbox
    } else if has("SENT") {
        Place::Sent
    } else if let Some(l) = labels.iter().find(|l| l.starts_with(FOLDER)) {
        Place::Folder {
            label: l.clone(),
            name: String::new(),
        }
    } else {
        Place::Archive
    }
}

/// What one message needs from Outlook.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub id: String,
    /// Folder to move it to, first.
    pub move_to: Option<String>,
    /// Fields to change (`isRead`, `categories`, `flag`), after the move.
    pub patch: Option<Value>,
    /// Its labels once both are done.
    pub labels_after: BTreeSet<String>,
}

/// The Outlook changes that make a thread's messages (provider id, stored labels) show
/// label names `add` added and `remove` removed:
/// - `TRASH` added moves to Deleted Items; removed, back to the Inbox (or Archive, or
///   the user folder added with it);
/// - `INBOX` added moves received mail to the Inbox (sent mail too when the thread has
///   nothing else); removed moves Inbox mail to Archive, or to the user folder of a
///   name added with it;
/// - a user folder's name added to archived mail files it there; removed from mail in
///   that folder moves it to Archive;
/// - any other name added or removed is a category; `UNREAD` and `STARRED` are read
///   state and flag.
///
/// Messages needing nothing are left out.
pub fn plan_thread(
    messages: &[(String, BTreeSet<String>)],
    add: &BTreeSet<String>,
    remove: &BTreeSet<String>,
    folders: &FolderMap,
) -> Vec<Edit> {
    let all_sent = messages
        .iter()
        .all(|(_, l)| place_from_labels(l) == Place::Sent);
    let folder_added = add.iter().find_map(|n| {
        folders
            .folder_named(n)
            .map(|id| (id.to_string(), n.clone()))
    });
    let inbox = folders.role("inbox").map(String::from);
    let archive = folders.role("archive").map(String::from);
    let trash = folders.role("deleteditems").map(String::from);
    let filed = || {
        folder_added
            .as_ref()
            .map(|(id, _)| id.clone())
            .or(archive.clone())
    };
    let mut out = vec![];
    for (id, labels) in messages {
        let cur = place_from_labels(labels);
        let cur_folder_name = match &cur {
            Place::Folder { label, .. } => FolderMap::folder_of_label(label)
                .and_then(|f| folders.place(f))
                .and_then(|p| match p {
                    Place::Folder { name, .. } => Some(name.clone()),
                    _ => None,
                }),
            _ => None,
        };
        let move_to = if add.contains("TRASH") {
            (cur != Place::Trash).then(|| trash.clone()).flatten()
        } else if cur == Place::Trash {
            if remove.contains("TRASH") {
                if add.contains("INBOX") {
                    inbox.clone()
                } else {
                    filed()
                }
            } else {
                None
            }
        } else if add.contains("INBOX") && matches!(cur, Place::Archive | Place::Folder { .. })
            || add.contains("INBOX") && cur == Place::Sent && all_sent
        {
            inbox.clone()
        } else if remove.contains("INBOX") && cur == Place::Inbox {
            filed()
        } else if let (Some((fid, _)), Place::Archive) = (&folder_added, &cur) {
            Some(fid.clone())
        } else if cur_folder_name.as_ref().is_some_and(|n| remove.contains(n)) {
            archive.clone()
        } else {
            None
        };
        // The folder the message ends in, for its labels and its categories.
        let moved_into_name = move_to.as_deref().and_then(|f| match folders.place(f) {
            Some(Place::Folder { name, .. }) => Some(name.clone()),
            _ => None,
        });
        let cats: BTreeSet<String> = labels
            .iter()
            .filter_map(|l| l.strip_prefix(CATEGORY).map(String::from))
            .collect();
        let mut new_cats = cats.clone();
        for n in add {
            if !SYSTEM.contains(&n.as_str()) && moved_into_name.as_ref() != Some(n) {
                new_cats.insert(n.clone());
            }
        }
        for n in remove {
            new_cats.remove(n);
        }
        let unread = labels.contains("UNREAD");
        let new_unread = if add.contains("UNREAD") {
            true
        } else if remove.contains("UNREAD") {
            false
        } else {
            unread
        };
        let starred = labels.contains("STARRED");
        let new_starred = if add.contains("STARRED") {
            true
        } else if remove.contains("STARRED") {
            false
        } else {
            starred
        };
        let mut patch = serde_json::Map::new();
        if new_cats != cats {
            patch.insert("categories".into(), json!(new_cats));
        }
        if new_unread != unread {
            patch.insert("isRead".into(), json!(!new_unread));
        }
        if new_starred != starred {
            let status = if new_starred { "flagged" } else { "notFlagged" };
            patch.insert("flag".into(), json!({ "flagStatus": status }));
        }
        if move_to.is_none() && patch.is_empty() {
            continue;
        }
        let place_after = match &move_to {
            Some(f) => folders.place(f).cloned(),
            None => Some(cur.clone()),
        };
        out.push(Edit {
            id: id.clone(),
            move_to,
            patch: (!patch.is_empty()).then_some(Value::Object(patch)),
            labels_after: labels_of(place_after.as_ref(), &new_cats, new_unread, new_starred),
        });
    }
    out
}

#[cfg(test)]
mod tests;
