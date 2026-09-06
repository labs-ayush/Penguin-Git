use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::get, Router};
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod auth;
mod db;
mod error;
mod models;
mod routes;

pub struct AppState {
    pub db: sqlx::PgPool,
    pub rate_limiter: auth::RateLimiter,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "penguingit_server=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/penguingit".into());

    tracing::info!("Connecting to database...");
    let pool = db::create_pool(&db_url).await?;

    tracing::info!("Running database migrations...");
    db::run_migrations(&pool).await?;

    let state = Arc::new(AppState {
        db: pool,
        rate_limiter: auth::RateLimiter::new(5, std::time::Duration::from_secs(10)),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route(
            "/api/auth/register",
            axum::routing::post(routes::auth::register),
        )
        .route(
            "/api/auth/login",
            axum::routing::post(routes::auth::login).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth::rate_limit_middleware,
            )),
        )
        .route(
            "/api/auth/logout",
            axum::routing::post(routes::auth::logout),
        )
        .route(
            "/api/patches",
            axum::routing::post(routes::patches::create_patch).get(routes::patches::list_patches),
        )
        .route(
            "/api/patches/:id",
            axum::routing::get(routes::patches::get_patch),
        )
        .route(
            "/api/patches/:id/comments",
            axum::routing::post(routes::patches::add_comment).get(routes::patches::list_comments),
        )
        .route(
            "/api/workspaces",
            axum::routing::post(routes::workspaces::create_workspace)
                .get(routes::workspaces::list_workspaces),
        )
        .route(
            "/api/workspaces/:id",
            axum::routing::get(routes::workspaces::get_workspace),
        )
        .route(
            "/api/workspaces/:id/members",
            axum::routing::post(routes::workspaces::add_member),
        )
        .route(
            "/api/workspaces/:id/members/:user_id",
            axum::routing::delete(routes::workspaces::remove_member),
        )
        .route(
            "/api/workspaces/:id/repos",
            axum::routing::post(routes::workspaces::add_repo),
        )
        .route(
            "/api/workspaces/:id/repos/:repo_name",
            axum::routing::delete(routes::workspaces::remove_repo),
        )
        .layer(cors)
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".into())
        .parse()?;

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("PenguinGit Cloud Server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
