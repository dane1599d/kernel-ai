//! Each OS row bound into a script's engine: the system rows, and the file
//! rows with their transaction.

#[allow(unused_imports)]
use super::*;

/// Binds the script API row `name`, with the arguments and result its row declares;
/// false if the shell has no implementation for it.
pub(super) fn bind(engine: &mut Engine, name: &'static str, tx: &Tx) -> bool {
    {
        let (h, tx) = (Rc::clone(&tx.host), tx.clone());
        match name {
            "cpus" => {
                engine.register_fn(name, move || -> RhaiResult<INT> {
                    tx.approved(name, &[])?;
                    Ok(metrics(h.as_ref())?.cpus as INT)
                });
            }
            "memory" => {
                engine.register_fn(name, move || -> RhaiResult<Map> {
                    tx.approved(name, &[])?;
                    let m = metrics(h.as_ref())?;
                    let mut map = Map::new();
                    map.insert(
                        "free_mib".into(),
                        ((m.free_frames * 4 / 1024) as INT).into(),
                    );
                    map.insert(
                        "total_mib".into(),
                        ((m.total_frames * 4 / 1024) as INT).into(),
                    );
                    map.insert("heap_kib".into(), ((m.heap_bytes / 1024) as INT).into());
                    Ok(map)
                });
            }
            "uptime_ms" => {
                engine.register_fn(name, move || -> RhaiResult<INT> {
                    tx.approved(name, &[])?;
                    Ok((metrics(h.as_ref())?.uptime_ns / 1_000_000) as INT)
                });
            }
            "processes" => {
                engine.register_fn(name, move || -> RhaiResult<Array> {
                    tx.approved(name, &[])?;
                    let m = metrics(h.as_ref())?;
                    let list = m.processes.iter().take(m.process_count as usize).map(|p| {
                        let mut map = Map::new();
                        map.insert("pid".into(), (p.pid as INT).into());
                        map.insert("name".into(), p.name().to_string().into());
                        // Pages of 4 KiB, 256 to the MiB.
                        map.insert("mib".into(), (p.pages.div_ceil(256) as INT).into());
                        map.insert("cpu_ms".into(), ((p.cpu_ns / 1_000_000) as INT).into());
                        Dynamic::from_map(map)
                    });
                    Ok(list.collect())
                });
            }
            "threads" => {
                engine.register_fn(name, move || -> RhaiResult<Map> {
                    tx.approved(name, &[])?;
                    let m = metrics(h.as_ref())?;
                    let mut map = Map::new();
                    map.insert("switches".into(), (m.context_switches as INT).into());
                    map.insert("ejections".into(), (m.ejections as INT).into());
                    map.insert("migrations".into(), (m.migrations as INT).into());
                    Ok(map)
                });
            }
            "get_policy" => {
                engine.register_fn(name, move || -> RhaiResult<String> {
                    tx.approved(name, &[])?;
                    Ok(policy_name(metrics(h.as_ref())?.policy).into())
                });
            }
            "set_policy" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    let id = match which {
                        "round-robin" | "rr" => policy::ROUND_ROBIN,
                        "interactive" => policy::INTERACTIVE,
                        other => return Err(rhai_error(format!("unknown policy {other:?}"))),
                    };
                    // No undo, no change: the tier promises one.
                    let (was, preset) = undo::current_spec(h.as_ref()).ok_or_else(|| {
                        rhai_error("set_policy: the installed policy could not be put back, so it was left as it is")
                    })?;
                    h.set_policy(id)
                        .map_err(|error| rhai_error(format!("set_policy: {error:?}")))?;
                    // What runs now, so `undo` can tell whether it moved since.
                    let after = undo::current_spec(h.as_ref()).unwrap_or((id, preset));
                    tx.changed(Done {
                        action: "set_policy".into(),
                        owner: "kernel".into(),
                        res: "policy".into(),
                        before: undo::spec_name(was, preset),
                        after: policy_name(id).into(),
                        inverse: Some(Inverse::Spec {
                            policy: was,
                            preset,
                            after,
                        }),
                    });
                    Ok(())
                });
            }
            "process_stop" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    let count = h
                        .stop_process(which, tx.consent.proof)
                        .map_err(|why| rhai_error(format!("process_stop: {why}")))?;
                    tx.changed(stopped(
                        name,
                        "kernel",
                        "process",
                        which,
                        &format!("stopped ({count})"),
                        None,
                    ));
                    Ok(())
                });
            }
            "service_stop" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    h.stop_service(which, tx.consent.proof)
                        .map_err(|why| rhai_error(format!("service_stop: {why}")))?;
                    tx.changed(stopped(
                        name,
                        "supervisor",
                        "service",
                        which,
                        "stopped",
                        None,
                    ));
                    Ok(())
                });
            }
            "service_restart" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    h.restart_service(which, tx.consent.proof)
                        .map_err(|why| rhai_error(format!("service_restart: {why}")))?;
                    tx.changed(stopped(
                        name,
                        "supervisor",
                        "service",
                        which,
                        "restarted",
                        None,
                    ));
                    Ok(())
                });
            }
            "service_start" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    // No undo, no change: the tier promises one.
                    let undo = undo::Name::new(which).ok_or_else(|| {
                        rhai_error("service_start: no service has a name that long")
                    })?;
                    h.start_service(which, tx.consent.entry)
                        .map_err(|why| rhai_error(format!("service_start: {why}")))?;
                    tx.changed(stopped(
                        name,
                        "supervisor",
                        "service",
                        which,
                        "running",
                        Some(Inverse::Started(undo)),
                    ));
                    Ok(())
                });
            }
            name if name.starts_with("fs_") => return bind_file(engine, name, h, tx),
            "appearance_set" => {
                look::bind(engine, h, tx);
            }
            name if agentabi::UI_ROWS.iter().any(|r| r.name == name) => {
                return windows::bind(engine, name, h, tx);
            }
            name if agentabi::GRANT_ROWS.iter().any(|r| r.name == name) => {
                return grants::bind(engine, name, h, tx);
            }
            name if agentabi::NET_ROWS.iter().any(|r| r.name == name) => {
                return net::bind(engine, name, h, tx);
            }
            _ => return false,
        }
    }
    true
}

/// Binds P2's file row `name` (task 11): reads hand back `Data`, and each change
/// is one `fs` transaction that `undo` restores.
pub(super) fn bind_file(engine: &mut Engine, name: &'static str, h: Rc<dyn Host>, tx: Tx) -> bool {
    let fail = move |why: String| rhai_error(format!("{name}: {why}"));
    match name {
        "fs_list" | "fs_read" | "fs_history" => {
            engine.register_fn(name, move |path: &str| -> RhaiResult<data::Data> {
                tx.approved(name, &[path])?;
                let path = home(name, path)?;
                let read = match name {
                    "fs_list" => FsRead::List(path),
                    "fs_read" => FsRead::Read(path),
                    _ => FsRead::History(path),
                };
                h.fs_read(read)
                    .map(|text| data::Data::new("fs", text))
                    .map_err(fail)
            });
        }
        "fs_find" => {
            engine.register_fn(name, move |word: &str| -> RhaiResult<data::Data> {
                tx.approved(name, &[word])?;
                h.fs_read(FsRead::Find(word))
                    .map(|text| data::Data::new("fs", text))
                    .map_err(fail)
            });
        }
        "fs_changes" => {
            engine.register_fn(name, move || -> RhaiResult<String> {
                tx.approved(name, &[])?;
                h.fs_read(FsRead::Changes).map_err(fail)
            });
        }
        "fs_create" | "fs_write" => {
            engine.register_fn(name, move |path: &str, text: &str| -> RhaiResult<()> {
                tx.approved(name, &[path, text])?;
                let path = home(name, path)?;
                let (change, before, after) = match name {
                    "fs_create" => (FsChange::Create { path, text }, "absent", "created"),
                    _ => (
                        FsChange::Write { path, text },
                        "its earlier text",
                        "written",
                    ),
                };
                let txn = h.fs_change(change).map_err(fail)?;
                tx.changed(file_done(name, path, before, after, Some(txn)));
                Ok(())
            });
        }
        "fs_move" => {
            engine.register_fn(name, move |from: &str, to: &str| -> RhaiResult<()> {
                tx.approved(name, &[from, to])?;
                let (from, to) = (home(name, from)?, home(name, to)?);
                let txn = h.fs_change(FsChange::Move { from, to }).map_err(fail)?;
                let after = format!("moved to {to}");
                tx.changed(file_done(name, from, "there", &after, Some(txn)));
                Ok(())
            });
        }
        "fs_trash" | "fs_restore" => {
            engine.register_fn(name, move |path: &str| -> RhaiResult<()> {
                tx.approved(name, &[path])?;
                let path = home(name, path)?;
                let (change, before, after) = match name {
                    "fs_trash" => (FsChange::Trash(path), "there", "in the trash"),
                    _ => (FsChange::Restore(path), "in the trash", "restored"),
                };
                let txn = h.fs_change(change).map_err(fail)?;
                tx.changed(file_done(name, path, before, after, Some(txn)));
                Ok(())
            });
        }
        "fs_empty_trash" => {
            engine.register_fn(name, move || -> RhaiResult<()> {
                tx.approved(name, &[])?;
                h.fs_change(FsChange::EmptyTrash).map_err(fail)?;
                tx.changed(file_done(name, "trash", "full", "emptied", None));
                Ok(())
            });
        }
        _ => return false,
    }
    true
}

/// `path` as the `fs` client takes it, relative to `/home/user`: a relative path
/// starts there, `/home/user/…` is the same, and any other absolute path
/// (`/system/log`) or a `..` is `None`. The AI's file rows reach only the user's
/// files, never `/system` (P2 plan §4.3); `fs`'s lookup refuses `..` too.
pub(crate) fn home_path(path: &str) -> Option<&str> {
    let rest = match path.strip_prefix('/') {
        None => path,
        Some(_) if path == "/home/user" => "",
        Some(_) => path.strip_prefix("/home/user/")?,
    };
    let escapes = rest.split('/').any(|part| part == "..");
    (!escapes).then_some(rest)
}

/// [`home_path`] for row `name`'s binding, refused in its words.
pub(super) fn home<'a>(name: &str, path: &'a str) -> RhaiResult<&'a str> {
    home_path(path).ok_or_else(|| rhai_error(format!("{name}: {path} is not under /home/user")))
}

/// One file change: `file notes.txt created (was absent)`, undone by restoring
/// transaction `txn`.
pub(crate) fn file_done(
    action: &'static str,
    path: &str,
    before: &str,
    after: &str,
    txn: Option<u64>,
) -> Done {
    Done {
        action: action.into(),
        owner: "fs".into(),
        res: format!("file {path}"),
        before: before.into(),
        after: after.into(),
        inverse: txn.map(|txn| Inverse::Fs { txn }),
    }
}

/// One process or service change: `service notes running (was stopped)`.
pub(super) fn stopped(
    action: &'static str,
    owner: &str,
    kind: &str,
    which: &str,
    after: &str,
    inverse: Option<Inverse>,
) -> Done {
    Done {
        action: action.into(),
        owner: owner.into(),
        res: format!("{kind} {which}"),
        before: String::from(if after == "running" {
            "stopped"
        } else {
            "running"
        }),
        after: after.into(),
        inverse,
    }
}
