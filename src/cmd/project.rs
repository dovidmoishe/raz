use anyhow::{bail, Result};

use crate::models::{Paths, State};
use crate::projects;

pub fn list() -> Result<()> {
    let state = load()?;
    println!("{}", projects::list_text(&state));
    Ok(())
}

pub fn add(name: &str) -> Result<()> {
    let paths = Paths::resolve()?;
    let mut state = State::load_or_init(&paths)?;
    ensure_workspace(&mut state, &paths)?;
    let msg = projects::register(&mut state, name, name)?;
    state.save(&paths.state)?;
    println!("{msg}");
    Ok(())
}

pub fn use_project(name: &str) -> Result<()> {
    let paths = Paths::resolve()?;
    let mut state = load_mut(&paths)?;
    let msg = projects::switch(&mut state, name)?;
    state.save(&paths.state)?;
    println!("{msg}");
    Ok(())
}

pub fn clone(owner_repo: &str, name: Option<&str>) -> Result<()> {
    let paths = Paths::resolve()?;
    let mut state = State::load_or_init(&paths)?;
    ensure_workspace(&mut state, &paths)?;
    let msg = projects::clone(&mut state, owner_repo, name)?;
    state.save(&paths.state)?;
    println!("{msg}");
    Ok(())
}

fn load() -> Result<State> {
    let paths = Paths::resolve()?;
    if !paths.state.exists() {
        bail!("no state.json — run raz setup");
    }
    State::load(&paths.state)
}

fn load_mut(paths: &Paths) -> Result<State> {
    if !paths.state.exists() {
        bail!("no state.json — run raz setup");
    }
    State::load(&paths.state)
}

fn ensure_workspace(state: &mut State, paths: &Paths) -> Result<()> {
    if state.workspace_root.is_empty() {
        state.workspace_root = paths.workspace.to_string_lossy().into_owned();
    }
    std::fs::create_dir_all(&state.workspace_root)?;
    Ok(())
}
