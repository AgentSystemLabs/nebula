use super::*;

impl Daemon {
    // ---- projects ----

    /// Register a repo as a project. One repo is one project: a path that
    /// resolves to a repo already registered — its root or any checkout of
    /// it — is refused.
    pub async fn add_project(
        self: &Arc<Self>,
        path: &Path,
        name: Option<String>,
        create_missing: bool,
    ) -> Result<EntityId> {
        if create_missing {
            if !path.exists() {
                tokio::fs::create_dir_all(path)
                    .await
                    .with_context(|| format!("create {}", path.display()))?;
            }
            // A project is a repository, so the confirmed folder becomes
            // one — unless it already sits inside one, or git is missing
            // and the check below should say so.
            match git::repo_toplevel(path).await {
                Err(e) if !git::is_missing(&e) => git::init(path).await?,
                _ => {}
            }
        }
        // "not a git repository" is the right explanation only when git ran and
        // said no — if git itself is missing, that message blames the wrong
        // thing, so let git.rs's own diagnosis through untouched.
        let toplevel = git::repo_toplevel(path).await.map_err(|e| {
            if git::is_missing(&e) {
                e
            } else {
                e.context(format!("{} is not a git repository", path.display()))
            }
        })?;
        // `--show-toplevel` answers with the checkout it was run in, so inside a
        // linked worktree it names the worktree rather than the repo. A project
        // is the repo: root it at the main checkout, which `git worktree list`
        // always puts first. Adding from inside a worktree used to name the
        // project after that worktree and leave its ⌂ root row pointing at a
        // directory the project did not own.
        let entries = git::list_worktrees(&toplevel)
            .await
            .with_context(|| format!("list checkouts of {}", toplevel.display()))?;
        let repo_path = match entries.first() {
            Some(main) => main.path.clone(),
            // git listing no checkout at all for a path it just called a work
            // tree would leave the root unknowable; refuse rather than seed a
            // project with no rows, which is how a project loses its root row.
            None => bail!("git listed no checkout for {}", toplevel.display()),
        };
        let duplicate_path = repo_path.clone();
        if self
            .store_blocking(move |store| store.project_by_path(&duplicate_path))
            .await?
            .is_some()
        {
            bail!("project already added: {}", repo_path.display());
        }
        let name = name.unwrap_or_else(|| Project::folder_name(&repo_path));
        let sort_order = self
            .store_blocking(|store| store.next_project_sort_order())
            .await?;
        let project = Project {
            id: ProjectId::generate(),
            name,
            repo_path: repo_path.clone(),
            sort_order,
        };
        let project_to_insert = project.clone();
        self.store_blocking(move |store| store.insert_project(&project_to_insert))
            .await?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project.clone()),
        });

        // Main checkout is modeled as a worktree row; adopt pre-existing
        // worktrees too so `nebula` matches reality on day one. Root-ness is
        // the path test the reconcile uses, not insert order — the two agreeing
        // is what keeps `repo_path` and the ⌂ root row the same directory.
        for entry in entries {
            let worktree = Worktree {
                id: WorktreeId::generate(),
                project_id: project.id.clone(),
                is_main: entry.path == repo_path,
                path: entry.path.clone(),
                branch: entry.branch,
                sort_order: 0,
            };
            let worktree_to_insert = worktree.clone();
            self.store_blocking(move |store| store.insert_worktree(&worktree_to_insert))
                .await?;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        Ok(EntityId::Project(project.id))
    }

    /// Retitle a project's row. Cosmetic only — the checkout on disk is never
    /// renamed, and every worktree under the project keeps its own path. An
    /// empty name resets the row to the folder's name, which is the only way
    /// back once a project has been renamed.
    pub fn rename_project(self: &Arc<Self>, id: &ProjectId, name: &str) -> Result<()> {
        let mut project = self.store.get_project(id)?.context("project not found")?;
        let name = name.trim();
        project.name = if name.is_empty() {
            Project::folder_name(&project.repo_path)
        } else {
            name.to_string()
        };
        self.store.rename_project(id, &project.name)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project),
        });
        Ok(())
    }

    /// Point a project at the folder its repo now lives in, after the user
    /// renamed or moved it on disk. Without this a moved repo is stranded:
    /// the row keeps the old path, and adding the new one makes a second
    /// project that shares none of the first one's sessions.
    ///
    /// `path` must be the repo's main checkout and not another project's.
    /// Every worktree row inside the old folder (the ⌂ root row, and any
    /// checkout nested in it) moves with it; a checkout outside it, such as
    /// a sibling `<repo>-worktrees/` one, keeps its path. git's own links
    /// are repaired both ways first, so a failed repair changes nothing. A
    /// row still named after the old folder takes the new folder's name; a
    /// renamed one keeps its name. Sessions follow their worktree rows, and
    /// one already running keeps running: its working directory moved with
    /// the folder.
    pub async fn set_project_path(self: &Arc<Self>, id: &ProjectId, path: &Path) -> Result<()> {
        let project_lookup = id.clone();
        let mut project = self
            .store_blocking(move |store| store.get_project(&project_lookup))
            .await?
            .context("project not found")?;
        let toplevel = git::repo_toplevel(path).await.map_err(|e| {
            if git::is_missing(&e) {
                e
            } else {
                e.context(format!("{} is not a git repository", path.display()))
            }
        })?;
        // The same rooting `add_project` does, except that a linked checkout
        // is refused rather than followed to its repo: the user named the
        // project's new home, and a worktree of it is not that.
        let entries = git::list_worktrees(&toplevel)
            .await
            .with_context(|| format!("list checkouts of {}", toplevel.display()))?;
        match entries.first() {
            Some(main) if main.path == toplevel => {}
            Some(main) => bail!(
                "{} is a worktree of {}; choose the repo's main checkout",
                toplevel.display(),
                main.path.display()
            ),
            None => bail!("git listed no checkout for {}", toplevel.display()),
        }
        let old = project.repo_path.clone();
        if toplevel == old {
            return Ok(());
        }
        let duplicate_path = toplevel.clone();
        if let Some(other) = self
            .store_blocking(move |store| store.project_by_path(&duplicate_path))
            .await?
        {
            if &other != id {
                let other_lookup = other.clone();
                let name = self
                    .store_blocking(move |store| store.get_project(&other_lookup))
                    .await?
                    .map(|p| p.name)
                    .unwrap_or_default();
                bail!(
                    "{} is already the project \"{name}\"; remove that one first",
                    toplevel.display()
                );
            }
        }

        // Held across the repair and the write, so the WORKTREE SYNC never
        // reconciles this project against half-moved rows.
        let ops = self.worktree_ops.lock().await;
        let (_, worktrees, _, _) = self.store_blocking(|store| store.load_tree()).await?;
        let moved: Vec<(WorktreeId, PathBuf)> = worktrees
            .iter()
            .filter(|w| &w.project_id == id)
            .filter_map(|w| {
                let rest = w.path.strip_prefix(&old).ok()?;
                Some((w.id.clone(), toplevel.join(rest)))
            })
            .collect();
        // git rejects a path that isn't there, so a nested checkout already
        // gone from disk is left for the sync to drop, as it would have been.
        let linked: Vec<PathBuf> = moved
            .iter()
            .map(|(_, p)| p.clone())
            .filter(|p| p != &toplevel && p.is_dir())
            .collect();
        git::repair_worktrees(&toplevel, &linked)
            .await
            .context("repair git's worktree links")?;
        if project.name == Project::folder_name(&old) {
            project.name = Project::folder_name(&toplevel);
        }
        project.repo_path = toplevel;
        let project_id = id.clone();
        let project_path = project.repo_path.clone();
        let project_name = project.name.clone();
        let moved_to_store = moved.clone();
        self.store_blocking(move |store| {
            store.set_project_path(&project_id, &project_path, &project_name, &moved_to_store)
        })
        .await?;
        drop(ops);

        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project.clone()),
        });
        for (wt, _) in &moved {
            let wt_lookup = wt.clone();
            if let Some(worktree) = self
                .store_blocking(move |store| store.get_worktree(&wt_lookup))
                .await?
            {
                self.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(worktree),
                });
            }
        }
        // Branch names and checkouts that changed while the project was
        // stranded: reconcile now rather than on the sync's next pass.
        if let Err(e) = self.sync_project_worktrees(&project).await {
            tracing::warn!(project = %project.name, error = %e, "worktree sync after a move failed");
        }
        Ok(())
    }

    pub fn remove_project(self: &Arc<Self>, id: &ProjectId) -> Result<()> {
        // Kill any live sessions under this project first.
        let (_, worktrees, agents, terminals) = self.store.load_tree()?;
        let wt_ids: Vec<WorktreeId> = worktrees
            .into_iter()
            .filter(|w| &w.project_id == id)
            .map(|w| w.id)
            .collect();
        self.kill_sessions_in(&wt_ids, &agents, &terminals);
        // Removing a project only forgets it in nebula — never touches disk.
        self.store.delete_project(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Project(id.clone()),
        });
        Ok(())
    }

    // ---- worktrees ----

    pub async fn create_worktree(
        self: &Arc<Self>,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<EntityId> {
        if branch.trim().is_empty() {
            bail!("branch name is empty");
        }
        let ops = self.worktree_ops.lock().await;
        let worktree = self.cut_worktree(project_id, branch, base).await?;
        drop(ops);
        Ok(EntityId::Worktree(worktree.id))
    }

    /// [`Self::create_worktree`]'s work, for a caller that already holds
    /// `worktree_ops`: cut the checkout, register its row, run the
    /// WORKTREE HOOK.
    pub(super) async fn cut_worktree(
        self: &Arc<Self>,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<Worktree> {
        let project_lookup = project_id.clone();
        let project = self
            .store_blocking(move |store| store.get_project(&project_lookup))
            .await?
            .context("project not found")?;
        // A base the caller named (`nebula worktree --base`) is resolved
        // against the fetched origin — `main` means `origin/main`, never
        // this checkout's local branch; every other new WORKTREE — `n` in
        // the WORKTREES PANEL, a bare `nebula worktree`, the QUICK PROMPT's
        // auto-created one — starts at the `worktree_base_branch` SETTING
        // when one is set (`master`, resolved the same way), else at the
        // fetched `origin/HEAD`; never at this checkout's HEAD.
        let path = match base {
            Some(base) => git::add_worktree_off_ref(&project.repo_path, branch, base).await?,
            None => match crate::config::Config::load().worktree_base_branch() {
                Some(configured) => {
                    git::add_worktree_off_configured(&project.repo_path, branch, configured).await?
                }
                None => git::add_worktree_off_default(&project.repo_path, branch).await?,
            },
        };
        let worktree = self.register_worktree(project_id, path, branch).await?;
        // The row is out; the WORKTREE HOOK runs still under the lock, so
        // it is ordered with the operation it belongs to — a delete of
        // this path waits for it, two hooks never overlap — and the Ack
        // waits for it, so whatever it provisions is in place before
        // anything is launched in the checkout. The hook timeout bounds
        // what that holds the lock for.
        self.run_worktree_hook(WorktreeHook::Create, &project.repo_path, &worktree)
            .await;
        Ok(worktree)
    }

    /// The PROJECT's worktree on `branch` — the ROOT WORKTREE when the
    /// branch is checked out there — or a new one cut from `base` (None:
    /// the configured base). Looked up under `worktree_ops`, as
    /// [`Self::pr_worktree`] does, so two requests for one new branch get
    /// one checkout instead of a race the second loses. What `nebula
    /// worktree` moves a session into and `nebula spawn --worktree` starts
    /// one in.
    pub(crate) async fn worktree_on_branch(
        self: &Arc<Self>,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<Worktree> {
        self.worktree_on_branch_inner(project_id, branch, base, false)
            .await
    }

    /// `nebula spawn --worktree --base` promises the new session starts from
    /// the named ref, so refuse existing branches instead of silently using
    /// their current history. Plain `nebula worktree --base` keeps its
    /// legacy fallback in [`Self::worktree_on_branch`].
    pub(crate) async fn worktree_on_branch_for_spawn(
        self: &Arc<Self>,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<Worktree> {
        self.worktree_on_branch_inner(project_id, branch, base, true)
            .await
    }

    pub(super) async fn worktree_on_branch_inner(
        self: &Arc<Self>,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
        refuse_existing_branch_with_base: bool,
    ) -> Result<Worktree> {
        if branch.trim().is_empty() {
            bail!("branch name is empty");
        }
        let ops = self.worktree_ops.lock().await;
        let (_, worktrees, _, _) = self.store_blocking(|store| store.load_tree()).await?;
        if let Some(existing) = worktrees
            .into_iter()
            .find(|w| &w.project_id == project_id && w.branch == branch)
        {
            if base.is_some() && refuse_existing_branch_with_base {
                bail!(
                    "branch `{branch}` already has a worktree at {}; --base only applies to a new \
                     branch — run it again without --base to use that worktree",
                    existing.path.display()
                );
            }
            return Ok(existing);
        }
        if let (Some(_), true) = (base, refuse_existing_branch_with_base) {
            let project_lookup = project_id.clone();
            let project = self
                .store_blocking(move |store| store.get_project(&project_lookup))
                .await?
                .context("project not found")?;
            if git::local_branch(&project.repo_path, branch).await {
                bail!(
                    "branch `{branch}` already exists; --base only applies to a new branch — run \
                     it again without --base to use the branch as it is"
                );
            }
        }
        let worktree = self.cut_worktree(project_id, branch, base).await?;
        drop(ops);
        Ok(worktree)
    }

    /// The checkout every PR SESSION for pull request `number` runs in: the
    /// PROJECT's worktree already on its head branch `head` (the ROOT
    /// WORKTREE only when the branch is checked out there — git allows a
    /// branch in one checkout at a time), or a new one under the WORKTREE
    /// DIR with the branch fetched from `origin` (`git::add_pr_worktree`).
    /// `head` is the checkout's branch as the client names it: a fork's
    /// arrives under its owner's name (`givemeurhats/main`), so a
    /// contributor's `main` never matches the ROOT WORKTREE on ours.
    /// Serialized with the other worktree ops, so two PR SESSIONS launched
    /// together get one checkout, not a race to create it.
    pub(crate) async fn pr_worktree(
        self: &Arc<Self>,
        project_id: &ProjectId,
        number: u64,
        head: &str,
    ) -> Result<Worktree> {
        let ops = self.worktree_ops.lock().await;
        let project_lookup = project_id.clone();
        let project = self
            .store_blocking(move |store| store.get_project(&project_lookup))
            .await?
            .context("project not found")?;
        let (_, worktrees, _, _) = self.store_blocking(|store| store.load_tree()).await?;
        if let Some(existing) = worktrees
            .into_iter()
            .find(|w| &w.project_id == project_id && w.branch == head)
        {
            return Ok(existing);
        }
        let path = git::add_pr_worktree(&project.repo_path, number, head).await?;
        let worktree = self.register_worktree(project_id, path, head).await?;
        self.run_worktree_hook(WorktreeHook::Create, &project.repo_path, &worktree)
            .await;
        drop(ops);
        Ok(worktree)
    }

    /// Record a checkout git just made as a worktree row and tell every
    /// client. Callers hold `worktree_ops`.
    pub(super) async fn register_worktree(
        self: &Arc<Self>,
        project_id: &ProjectId,
        path: PathBuf,
        branch: &str,
    ) -> Result<Worktree> {
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project_id.clone(),
            path,
            branch: branch.to_string(),
            is_main: false,
            sort_order: 0,
        };
        let worktree_to_insert = worktree.clone();
        self.store_blocking(move |store| store.insert_worktree(&worktree_to_insert))
            .await?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Worktree(worktree.clone()),
        });
        Ok(worktree)
    }

    pub async fn delete_worktree(self: &Arc<Self>, id: &WorktreeId, force: bool) -> Result<()> {
        let ops = self.worktree_ops.lock().await;
        let worktree_lookup = id.clone();
        let worktree = self
            .store_blocking(move |store| store.get_worktree(&worktree_lookup))
            .await?
            .context("worktree not found")?;
        if worktree.is_main {
            bail!("cannot delete the main checkout — remove the project instead");
        }
        let project_lookup = worktree.project_id.clone();
        let project = self
            .store_blocking(move |store| store.get_project(&project_lookup))
            .await?
            .context("project not found")?;

        // Kill sessions living in this worktree.
        let (_, _, agents, terminals) = self.store_blocking(|store| store.load_tree()).await?;
        self.kill_sessions_in(std::slice::from_ref(id), &agents, &terminals);

        git::remove_worktree(&project.repo_path, &worktree.path, force).await?;
        let delete_id = id.clone();
        self.store_blocking(move |store| store.delete_worktree(&delete_id))
            .await?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Worktree(id.clone()),
        });
        // The delete has happened as far as git and every client are
        // concerned; the WORKTREE HOOK only releases what the checkout
        // owned elsewhere, so it runs after, and its failure is a warning
        // — never an Error for this request, which would put the rows
        // back in the TUI. Still under the lock: a create of the same path
        // waits until the hook has released what it is about to claim,
        // and the hook's "still on disk" check sees the delete's result,
        // not a recreate's.
        self.run_worktree_hook(WorktreeHook::Delete, &project.repo_path, &worktree)
            .await;
        drop(ops);
        Ok(())
    }

    /// Run the repository's WORKTREE HOOK for `hook`, if it configures
    /// one, and turn anything it has to say into a client warning.
    pub(super) async fn run_worktree_hook(
        &self,
        hook: WorktreeHook,
        repo: &Path,
        worktree: &Worktree,
    ) {
        let ctx = HookContext {
            repo,
            worktree: &worktree.path,
            branch: &worktree.branch,
            id: &worktree.id,
        };
        if let Err(e) = worktree_hooks::run(hook, ctx).await {
            self.warn_clients(format!("{e:#}"));
        }
    }

    /// Tell every client about something that went wrong after a request
    /// had already succeeded. Rides `ServerEvent::Error` with no `req_id`,
    /// which the TUI shows as a flash and ties to no pending intent.
    pub(super) fn warn_clients(&self, message: String) {
        tracing::warn!("{message}");
        self.broadcast(ServerEvent::Error {
            req_id: None,
            message,
        });
    }

    /// Reconcile a project's worktree rows with `git worktree list` so
    /// checkouts made outside nebula (an agent running `git worktree add`,
    /// manual CLI use) appear without a restart. Adopts unknown checkouts;
    /// refreshes the branch on known rows after an in-place checkout;
    /// drops rows whose checkout vanished — except the main row and rows
    /// that still have sessions, which the user must delete deliberately.
    pub async fn sync_project_worktrees(self: &Arc<Self>, project: &Project) -> Result<()> {
        let adopted = {
            let _ops = self.worktree_ops.lock().await;
            self.reconcile_project_worktrees(project).await?
        };
        // Outside the ops lock: the replay only touches agent rows, and a
        // just-adopted checkout is exactly where a session that ran
        // `git worktree add` itself already lives.
        if adopted {
            self.reparent_agents_by_last_cwd(project);
        }
        Ok(())
    }

    /// The reconcile half of `sync_project_worktrees`. Returns whether any
    /// checkout was newly adopted.
    pub(super) async fn reconcile_project_worktrees(
        self: &Arc<Self>,
        project: &Project,
    ) -> Result<bool> {
        let mut adopted = false;
        let entries = git::list_worktrees(&project.repo_path).await?;
        // git lists the main checkout first, and that — not the order rows
        // happened to be inserted in — is what makes a row the ⌂ root row.
        // Deriving it here every pass repairs a project whose rows were seeded
        // before the root was known, and keeps root-ness following the repo
        // when the checkouts underneath it change.
        let main_path = entries.first().map(|e| e.path.clone());
        let is_root = |path: &Path| main_path.as_deref() == Some(path);
        let (_, worktrees, agents, terminals) =
            self.store_blocking(|store| store.load_tree()).await?;
        let ours: Vec<&Worktree> = worktrees
            .iter()
            .filter(|w| w.project_id == project.id)
            .collect();
        for entry in &entries {
            if let Some(known) = ours.iter().find(|w| w.path == entry.path) {
                // Branch switched in place (checkout on the root or inside a
                // linked worktree): refresh the stored name so the row tracks
                // reality instead of the branch at adoption time.
                let root = is_root(&entry.path);
                if known.branch != entry.branch || known.is_main != root {
                    if known.branch != entry.branch {
                        let id = known.id.clone();
                        let branch = entry.branch.clone();
                        self.store_blocking(move |store| {
                            store.update_worktree_branch(&id, &branch)
                        })
                        .await?;
                    }
                    if known.is_main != root {
                        let id = known.id.clone();
                        self.store_blocking(move |store| store.set_worktree_main(&id, root))
                            .await?;
                    }
                    let mut updated = (*known).clone();
                    updated.branch = entry.branch.clone();
                    updated.is_main = root;
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(updated),
                    });
                }
                continue;
            }
            let worktree = Worktree {
                id: WorktreeId::generate(),
                project_id: project.id.clone(),
                is_main: is_root(&entry.path),
                path: entry.path.clone(),
                branch: entry.branch.clone(),
                sort_order: 0,
            };
            let worktree_to_insert = worktree.clone();
            self.store_blocking(move |store| store.insert_worktree(&worktree_to_insert))
                .await?;
            adopted = true;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        for w in ours {
            // The main checkout is always somewhere in git's list, so a row
            // that isn't there is a linked checkout that went away — including
            // one still carrying an `is_main` from before root-ness was
            // derived, which no longer earns the row a reprieve.
            if entries.iter().any(|e| e.path == w.path) {
                continue;
            }
            let occupied = agents.iter().any(|a| a.worktree_id == w.id)
                || terminals.iter().any(|t| t.worktree_id == w.id);
            if occupied {
                continue;
            }
            let id = w.id.clone();
            self.store_blocking(move |store| store.delete_worktree(&id))
                .await?;
            self.broadcast(ServerEvent::EntityRemoved {
                id: EntityId::Worktree(w.id.clone()),
            });
        }
        Ok(adopted)
    }
}
