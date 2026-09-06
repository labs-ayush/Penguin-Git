use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::ApiError,
    models::{Patch, PatchComment},
    routes::workspaces::check_workspace_membership,
    AppState,
};

#[derive(Debug, Deserialize)]
pub struct CreatePatchRequest {
    pub workspace_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub patch_data: String,
    pub repo_name: Option<String>,
    pub base_commit: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCommentRequest {
    pub body: String,
}

/// Helper to check if a user has access to read or comment on a patch.
pub async fn check_patch_access(
    db: &sqlx::PgPool,
    patch_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Patch>, ApiError> {
    let patch = sqlx::query_as::<_, Patch>(
        "SELECT id, workspace_id, author_id, title, description, patch_data, repo_name, base_commit, created_at
         FROM patches
         WHERE id = $1",
    )
    .bind(patch_id)
    .fetch_optional(db)
    .await?;

    let patch = match patch {
        Some(p) => p,
        None => return Ok(None),
    };

    if patch.author_id == user_id {
        return Ok(Some(patch));
    }

    if let Some(ws_id) = patch.workspace_id {
        let membership_info = check_workspace_membership(db, ws_id, user_id).await?;
        if membership_info.is_member {
            return Ok(Some(patch));
        }
    }

    Err(ApiError::NotFound("Patch not found".into()))
}

pub async fn create_patch(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(payload): Json<CreatePatchRequest>,
) -> Result<(StatusCode, Json<Patch>), ApiError> {
    if payload.title.trim().is_empty() || payload.patch_data.trim().is_empty() {
        return Err(ApiError::BadRequest("Title and patch_data required".into()));
    }

    if let Some(ws_id) = payload.workspace_id {
        let membership_info = check_workspace_membership(&state.db, ws_id, auth_user.id).await?;
        if !membership_info.is_member {
            return Err(ApiError::Forbidden(
                "You are not a member of this workspace".into(),
            ));
        }
    }

    let patch = sqlx::query_as::<_, Patch>(
        "INSERT INTO patches (workspace_id, author_id, title, description, patch_data, repo_name, base_commit)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, workspace_id, author_id, title, description, patch_data, repo_name, base_commit, created_at",
    )
    .bind(payload.workspace_id)
    .bind(auth_user.id)
    .bind(payload.title.trim())
    .bind(payload.description.as_deref().map(|s| s.trim()))
    .bind(&payload.patch_data)
    .bind(payload.repo_name.as_deref().map(|s| s.trim()))
    .bind(payload.base_commit.as_deref().map(|s| s.trim()))
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(patch)))
}

pub async fn list_patches(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
) -> Result<Json<Vec<Patch>>, ApiError> {
    let patches = sqlx::query_as::<_, Patch>(
        "SELECT DISTINCT p.id, p.workspace_id, p.author_id, p.title, p.description, p.patch_data, p.repo_name, p.base_commit, p.created_at
         FROM patches p
         LEFT JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
         LEFT JOIN workspaces w ON p.workspace_id = w.id
         WHERE p.author_id = $1 OR w.owner_id = $1 OR wm.user_id = $1
         ORDER BY p.created_at DESC",
    )
    .bind(auth_user.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(patches))
}

pub async fn get_patch(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Patch>, ApiError> {
    let patch = check_patch_access(&state.db, id, auth_user.id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Patch not found".into()))?;

    Ok(Json(patch))
}

pub async fn add_comment(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<CreateCommentRequest>,
) -> Result<(StatusCode, Json<PatchComment>), ApiError> {
    if payload.body.trim().is_empty() {
        return Err(ApiError::BadRequest("Comment body cannot be empty".into()));
    }

    let _patch = check_patch_access(&state.db, id, auth_user.id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Patch not found".into()))?;

    let comment = sqlx::query_as::<_, PatchComment>(
        "INSERT INTO patch_comments (patch_id, author_id, body)
         VALUES ($1, $2, $3)
         RETURNING id, patch_id, author_id, body, created_at",
    )
    .bind(id)
    .bind(auth_user.id)
    .bind(payload.body.trim())
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(comment)))
}

pub async fn list_comments(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<PatchComment>>, ApiError> {
    let _patch = check_patch_access(&state.db, id, auth_user.id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Patch not found".into()))?;

    let comments = sqlx::query_as::<_, PatchComment>(
        "SELECT id, patch_id, author_id, body, created_at
         FROM patch_comments
         WHERE patch_id = $1
         ORDER BY created_at ASC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(comments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_authorization_check_contract() {
        assert_eq!(
            ApiError::NotFound("Patch not found".into()).to_string(),
            "Not Found: Patch not found"
        );
        assert_eq!(
            ApiError::Forbidden("You are not a member of this workspace".into()).to_string(),
            "Forbidden: You are not a member of this workspace"
        );
    }
}
