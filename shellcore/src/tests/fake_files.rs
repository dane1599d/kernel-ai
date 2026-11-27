//! The test host's file calls: `fs`'s reads, changes and undos on [`FakeHost`]'s
//! files, as the Host methods of the same names give them.

use super::*;
use alloc::collections::BTreeSet;

impl FakeHost {
    pub(super) fn fake_fs_read(&self, read: FsRead) -> Result<String, String> {
        let files = self.files.borrow();
        match read {
            FsRead::List(dir) => Ok(files
                .keys()
                .filter(|path| path.starts_with(dir))
                .cloned()
                .collect::<Vec<_>>()
                .join(
                    "
",
                )),
            FsRead::Find(word) => Ok(files
                .keys()
                .filter(|path| {
                    let last = path.rsplit('/').next().unwrap_or(path);
                    last.to_lowercase().contains(&word.to_lowercase())
                })
                // As the shell's `find` answers: whole paths under home.
                .map(|path| format!("/home/user/{path}"))
                .collect::<Vec<_>>()
                .join("\n")),
            FsRead::Read(path) | FsRead::Whole(path) => files
                .get(path)
                .cloned()
                .ok_or_else(|| String::from("NotFound")),
            FsRead::History(path) => Ok(format!("{path}: {} versions", self.txns.borrow().len())),
            FsRead::Changes => Ok(files.keys().cloned().collect::<Vec<_>>().join(
                "
",
            )),
        }
    }

    pub(super) fn fake_fs_change(&self, change: FsChange) -> Result<u64, String> {
        if let Some(refused) = self.dropped_or_refused() {
            return refused;
        }
        if let Some(why) = self.fs_fail.borrow_mut().take() {
            return Err(why);
        }
        let before = self.files.borrow().clone();
        let mut files = before.clone();
        match change {
            FsChange::Create { path, .. } if files.contains_key(path) => {
                return Err(String::from("Exists: use fs_write"));
            }
            FsChange::Create { path, text }
            | FsChange::Write { path, text }
            | FsChange::Save { path, text, .. } => {
                files.insert(path.into(), text.into());
            }
            FsChange::Move { from, to } => {
                let text = files.remove(from).ok_or_else(|| String::from("NotFound"))?;
                files.insert(to.into(), text);
            }
            FsChange::Trash(path) => {
                let text = files.remove(path).ok_or_else(|| String::from("NotFound"))?;
                files.insert(format!(".trash/{path}"), text);
            }
            FsChange::Restore(path) => {
                let text = files
                    .remove(&format!(".trash/{path}"))
                    .ok_or_else(|| String::from("NotFound"))?;
                files.insert(path.into(), text);
            }
            FsChange::EmptyTrash => files.retain(|path, _| !path.starts_with(".trash/")),
        }
        *self.files.borrow_mut() = files.clone();
        let mut txns = self.txns.borrow_mut();
        if let Some(open) = self.open_txn.get() {
            txns[open].1 = files;
            return Ok(open as u64 + 1);
        }
        txns.push((before, files));
        if self.one_action.get() {
            self.open_txn.set(Some(txns.len() - 1));
            self.with_ledger(|l| l.begin(txns.len() as u64));
        }
        Ok(txns.len() as u64)
    }

    pub(super) fn fake_fs_undo(&self, txn: u64) -> Result<FsUndone, FsRefused> {
        if let Some(undone) = self.undo_lost(txn) {
            return undone;
        }
        let (before, after) = self
            .txns
            .borrow()
            .get(txn as usize - 1)
            .cloned()
            .ok_or_else(|| FsRefused::Failed(String::from("NotFound")))?;
        let now = self.files.borrow().clone();
        // The line's own open transaction is dropped, not restored.
        if self.open_txn.get() == Some(txn as usize - 1) {
            self.open_txn.set(None);
            self.with_ledger(|l| l.undo(txn));
            *self.files.borrow_mut() = before;
            return Ok(FsUndone {
                generation: txn,
                left: 0,
            });
        }
        let touched: BTreeSet<&String> = before
            .keys()
            .chain(after.keys())
            .filter(|path| before.get(*path) != after.get(*path))
            .collect();
        let mut files = now.clone();
        let (mut left, mut restored) = (0, false);
        for path in touched {
            match (before.get(path), now.get(path)) {
                (was, is) if is == after.get(path) => {
                    match was {
                        Some(text) => files.insert(path.clone(), text.clone()),
                        None => files.remove(path),
                    };
                    restored = true;
                }
                (Some(was), Some(_)) => {
                    files.insert(format!("{path} (restored)"), was.clone());
                    restored = true;
                }
                _ => left += 1,
            }
        }
        if !restored && left > 0 {
            return Err(FsRefused::Kept(format!(
                "{left} file(s) changed since, and fs never restores over a later change"
            )));
        }
        *self.files.borrow_mut() = files.clone();
        let mut txns = self.txns.borrow_mut();
        txns.push((now, files));
        Ok(FsUndone {
            generation: txns.len() as u64,
            left,
        })
    }
}
