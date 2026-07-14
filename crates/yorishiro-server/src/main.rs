use anyhow::Result;
use clap::Parser;
use yorishiro_core::db::TenantDb;
use yorishiro_server::admin::{self, AdminCommand};
use yorishiro_server::{AppState, build_app, build_embedding_provider, logging, shutdown_signal};

mod config;

/// A plain start (`yorishiro-server`, no subcommand) runs the HTTP server; `yorishiro-server
/// admin ...` runs one-off administrative commands instead.
#[derive(Parser)]
#[command(name = "yorishiro-server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Tenant and API key management, embedding resync.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
}

fn main() -> Result<()> {
    // Synchronous prologue: `config::load_and_apply_env_overrides` and the `YORISHIRO_MAX_TENANTS`
    // default both call `std::env::set_var`, which is unsound under concurrent env access. Doing
    // both here, before the tokio runtime (and its worker threads) starts, is what makes them
    // sound -- nothing else touches the environment yet.
    //
    // SAFETY: no other thread exists at this point in `main`.
    unsafe {
        config::load_and_apply_env_overrides()?;
        // Community-edition default: a single-tenant deployment unless the operator opts into
        // more (set YORISHIRO_MAX_TENANTS=0 for unlimited, or a higher count). Set before parsing
        // the CLI so `admin create-tenant` enforces the same default as the HTTP server.
        if std::env::var_os("YORISHIRO_MAX_TENANTS").is_none() {
            std::env::set_var("YORISHIRO_MAX_TENANTS", "1");
        }
    }

    let cli = Cli::parse();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    // The admin CLI prints for a human, so branch to it before initializing JSON-formatted
    // tracing.
    if let Some(Command::Admin { command }) = cli.command {
        return admin::run(command).await;
    }

    // Held for the rest of `main` because dropping it would stop the background thread that
    // the `single`/`daily` file targets flush through.
    let _log_guard = logging::init()?;

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let bind_addr = std::env::var("YSR_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());

    // Migrations need admin privileges (CREATE ROLE/GRANT/ALTER TABLE, etc.), so they run on
    // the admin pool, before request handling switches to the RLS-enforced role via
    // `SET ROLE`. That admin pool is kept (as `identity_pool`) rather than dropped: a handful
    // of control-plane endpoints (signup/login/invite redemption) need it too, since they
    // touch `identity.users`/`identity.tenant_memberships`/`identity.invites` before any
    // tenant/workspace context exists to scope `yorishiro_app`'s RLS-restricted access by (see
    // `AppState::identity_pool`'s doc comment). Everything else goes through `tenant_db`.
    let identity_pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::migrate!("../../migrations")
        .run(&identity_pool)
        .await?;

    let tenant_db = TenantDb::connect(&database_url, 20).await?;
    let embedding_provider = build_embedding_provider()?;
    let state = AppState::new(tenant_db, identity_pool, embedding_provider);
    let embedding_tasks = state.embedding_tasks().clone();
    let web_dir = Some(std::env::var("YSR_WEB_DIR").unwrap_or_else(|_| "web".into()));
    let app = build_app(state, web_dir);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("listening on {bind_addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    // After closing HTTP, wait for the embedding sync of already-written entities to finish.
    // Exiting immediately without waiting would leave recently created entities permanently
    // missing from search (recoverable via `admin resync-embeddings`, but the goal is to
    // avoid needing that on routine deploys). A second Ctrl-C/SIGTERM during this wait forces
    // an immediate exit instead -- without it, an operator who interrupts again out of
    // impatience would see no response at all until the 30s timeout, since the first signal's
    // `tokio::signal::ctrl_c()` in `shutdown_signal` has already resolved and nothing else is
    // listening for a repeat.
    embedding_tasks.close();
    tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(30), embedding_tasks.wait()) => {
            if result.is_err() {
                tracing::warn!(
                    "embedding syncs did not finish within 30s; exiting anyway \
                     (recover with `admin resync-embeddings`)"
                );
            }
        }
        _ = shutdown_signal() => {
            tracing::warn!(
                "second interrupt received; exiting immediately without waiting for embedding \
                 syncs to finish (recover with `admin resync-embeddings`)"
            );
        }
    }

    Ok(())
}
