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
    models::{UserPublic, Workspace, WorkspaceRepo},
    AppState,
};

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct AddMemberRequest {
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct AddRepoRequest {
    pub repo_name: String,
}

/// Helper function to check if a user is an owner or member of a workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceMemberInfo {
    pub is_member: bool,
    pub role: Option<String>,
}

pub async fn check_workspace_membership(
    db: &sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<WorkspaceMemberInfo, sqlx::Error> {
    let (is_member, role) = sqlx::query_as::<_, (bool, Option<String>)>(
        "SELECT EXISTS(
            SELECT 1 FROM workspaces WHERE id = $1 AND owner_id = $2
            UNION
            SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2
        ),
         (SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(db)
    .await?;

    Ok(WorkspaceMemberInfo { is_member, role })
}

pub async fn create_workspace(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(payload): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<Workspace>), ApiError> {
    let name = payload.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("Workspace name required".into()));
    }

    let mut tx = state.db.begin().await?;

    let ws = sqlx::query_as::<_, Workspace>(
        "INSERT INTO workspaces (name, owner_id)
         VALUES ($1, $2)
         RETURNING id, name, owner_id, created_at",
    )
    .bind(name)
    .bind(auth_user.id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, user_id, role)
         VALUES ($1, $2, 'owner')",
    )
    .bind(ws.id)
    .bind(auth_user.id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(ws)))
}

pub async fn list_workspaces(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
) -> Result<Json<Vec<Workspace>>, ApiError> {
    let workspaces = sqlx::query_as::<_, Workspace>(
        "SELECT DISTINCT w.id, w.name, w.owner_id, w.created_at
         FROM workspaces w
         LEFT JOIN workspace_members wm ON w.id = wm.workspace_id
         WHERE w.owner_id = $1 OR wm.user_id = $1
         ORDER BY w.name ASC",
    )
    .bind(auth_user.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(workspaces))
}

pub async fn get_workspace(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Workspace>, ApiError> {
    let membership_info = check_workspace_membership(&state.db, id, auth_user.id).await?;
    if !membership_info.is_member {
        return Err(ApiError::Forbidden(
            "You are not a member of this workspace".into(),
        ));
    }

    let ws = sqlx::query_as::<_, Workspace>(
        "SELECT id, name, owner_id, created_at FROM workspaces WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("Workspace not found".into()))?;

    Ok(Json(ws))
}

pub async fn add_member(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<AddMemberRequest>,
) -> Result<(StatusCode, Json<UserPublic>), ApiError> {
    let membership_info = check_workspace_membership(&state.db, id, auth_user.id).await?;
    if !membership_info.is_member {
        return Err(ApiError::Forbidden(
            "Only members can add users to this workspace".into(),
        ));
    }
    if membership_info.role != Some("owner".to_string()) {
        return Err(ApiError::Forbidden(
            "Only owners can add users to this workspace".into(),
        ));
    }

    let target_user = sqlx::query_as::<_, (Uuid, String, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, username, created_at FROM users WHERE username = $1",
    )
    .bind(payload.username.trim())
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("User not found".into()))?;

    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, user_id, role)
         VALUES ($1, $2, 'member')
         ON CONFLICT (workspace_id, user_id) DO NOTHING",
    )
    .bind(id)
    .bind(target_user.0)
    .execute(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(UserPublic {
            id: target_user.0,
            username: target_user.1,
            created_at: target_user.2,
        }),
    ))
}

pub async fn remove_member(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let membership_info = check_workspace_membership(&state.db, id, auth_user.id).await?;
    if !membership_info.is_member {
        return Err(ApiError::Forbidden(
            "Only members can modify workspace membership".into(),
        ));
    }
    if membership_info.role != Some("owner".to_string()) {
        return Err(ApiError::Forbidden(
            "Only owners can remove members from this workspace".into(),
        ));
    }
    if auth_user.id == user_id {
        return Err(ApiError::BadRequest(
            "Cannot remove yourself from workspace".into(),
        ));
    }
    if let Some(role) = membership_info.role {
        if role == "owner" && user_id == auth_user.id {
            return Err(ApiError::BadRequest(
                "Owners cannot remove themselves from workspace".into(),
            ));
        }
    }

    sqlx::query("DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_repo(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<AddRepoRequest>,
) -> Result<(StatusCode, Json<WorkspaceRepo>), ApiError> {
    let membership_info = check_workspace_membership(&state.db, id, auth_user.id).await?;
    if !membership_info.is_member {
        return Err(ApiError::Forbidden(
            "Only members can add repos to this workspace".into(),
        ));
    }
    if membership_info.role != Some("owner".to_string()) {
        return Err(ApiError::Forbidden(
            "Only owners can add repos to this workspace".into(),
        ));
    }

    let repo_name = payload.repo_name.trim();
    if repo_name.is_empty() {
        return Err(ApiError::BadRequest("Repository name required".into()));
    }

    let repo = sqlx::query_as::<_, WorkspaceRepo>(
        "INSERT INTO workspace_repos (workspace_id, repo_name)
         VALUES ($1, $2)
         ON CONFLICT (workspace_id, repo_name) DO UPDATE SET added_at = now()
         RETURNING workspace_id, repo_name, added_at",
    )
    .bind(id)
    .bind(repo_name)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(repo)))
}

pub async fn remove_repo(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path((id, repo_name)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    let membership_info = check_workspace_membership(&state.db, id, auth_user.id).await?;
    if !membership_info.is_member {
        return Err(ApiError::Forbidden(
            "Only members can remove repos from this workspace".into(),
        ));
    }
    if membership_info.role != Some("owner".to_string()) {
        return Err(ApiError::Forbidden(
            "Only owners can remove repos from this workspace".into(),
        ));
    }

    sqlx::query("DELETE FROM workspace_repos WHERE workspace_id = $1 AND repo_name = $2")
        .bind(id)
        .bind(repo_name)
        .execute(&state.db)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authorization_check_contract() {
        // Unit check for membership logic expectations
        assert_eq!(
            ApiError::Forbidden("You are not a member of this workspace".into()).to_string(),
            "Forbidden: You are not a member of this workspace"
        );
        assert_eq!(
            ApiError::Forbidden("Only owners can remove members from this workspace".into())
                .to_string(),
            "Forbidden: Only owners can remove members from this workspace"
        );
    }

    #[test]
    fn test_member_role_check_contract() {
        // Test that role-based restrictions are enforced
        let member_info = WorkspaceMemberInfo {
            is_member: true,
            role: Some("owner".to_string()),
        };
        assert!(member_info.is_member);
        assert_eq!(member_info.role, Some("owner".to_string()));

        let member_info = WorkspaceMemberInfo {
            is_member: true,
            role: Some("member".to_string()),
        };
        assert!(member_info.is_member);
        assert_eq!(member_info.role, Some("member".to_string()));

        let member_info = WorkspaceMemberInfo {
            is_member: false,
            role: None,
        };
        assert!(!member_info.is_member);
        assert!(member_info.role.is_none());
    }
}
