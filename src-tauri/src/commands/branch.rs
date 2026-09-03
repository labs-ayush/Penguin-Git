use std::path::Path;
use tauri::State;

use super::to_ipc_error;
use crate::core::branch::{self, Branch};
use crate::core::exec::run_git;
use crate::core::repo::AppState;
use crate::core::undo::ActionType;

#[tauri::command]
pub fn get_branches(repo_path: String) -> Result<Vec<Branch>, String> {
    branch::list_branches(Path::new(&repo_path)).map_err(to_ipc_error)
}

#[tauri::command]
pub fn create_branch(
    repo_path: String,
    name: String,
    start_point: Option<String>,
) -> Result<(), String> {
    branch::create_branch(Path::new(&repo_path), &name, start_point.as_deref())
        .map_err(to_ipc_error)
}

#[tauri::command]
pub fn delete_branch(
    state: State<'_, AppState>,
    repo_path: String,
    name: String,
    force: bool,
) -> Result<(), String> {
    branch::reject_option_like(&name).map_err(to_ipc_error)?;

    let p = Path::new(&repo_path);
    let target_hash = run_git(p, &["rev-parse", &name])
        .unwrap_or_default()
        .trim()
        .to_string();

    branch::delete_branch(p, &name, force).map_err(to_ipc_error)?;

    state.get_journal(&repo_path).record(
        ActionType::BranchDelete {
            branch_name: name.clone(),
            target_hash,
        },
        format!("Delete branch {name}"),
    );

    Ok(())
}

#[tauri::command]
pub fn rename_branch(repo_path: String, old: String, new: String) -> Result<(), String> {
    branch::rename_branch(Path::new(&repo_path), &old, &new).map_err(to_ipc_error)
}

#[tauri::command]
pub fn checkout(
    state: State<'_, AppState>,
    repo_path: String,
    target: String,
) -> Result<(), String> {
    let p = Path::new(&repo_path);
    let previous_ref = run_git(p, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();

    branch::checkout(p, &target).map_err(to_ipc_error)?;

    state.get_journal(&repo_path).record(
        ActionType::Checkout { previous_ref },
        format!("Checkout {target}"),
    );

    Ok(())
}

#[tauri::command]
pub fn checkout_new_branch(
    state: State<'_, AppState>,
    repo_path: String,
    name: String,
    start_point: Option<String>,
) -> Result<(), String> {
    let p = Path::new(&repo_path);
    let previous_ref = run_git(p, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();

    branch::checkout_new(p, &name, start_point.as_deref()).map_err(to_ipc_error)?;

    state.get_journal(&repo_path).record(
        ActionType::Checkout { previous_ref },
        format!("Checkout new branch {name}"),
    );

    Ok(())
}

#[tauri::command]
pub fn merge_branch(
    state: State<'_, AppState>,
    repo_path: String,
    branch_name: String,
) -> Result<(), String> {
    let p = Path::new(&repo_path);
    let previous_head = run_git(p, &["rev-parse", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();

    let res = branch::merge_branch(p, &branch_name).map_err(to_ipc_error);

    let state_detector = crate::core::merge_state::detect_operation_state(p);
    if res.is_ok() || state_detector.kind == Some(crate::core::merge_state::OperationKind::Merge) {
        state.get_journal(&repo_path).record(
            ActionType::Merge {
                previous_head,
                target_ref: branch_name.clone(),
            },
            format!("Merge branch {branch_name}"),
        );
    }

    res
}

#[tauri::command]
pub fn rebase_onto(repo_path: String, onto: String) -> Result<(), String> {
    branch::rebase_onto(Path::new(&repo_path), &onto).map_err(to_ipc_error)
}
