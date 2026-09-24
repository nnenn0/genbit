use crate::route::{Route, is_reserved_url};
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
    response::sse::{Event, Sse},
    routing::get,
};
use notify::{EventKind, RecursiveMode, Watcher};
use std::{
    convert::Infallible,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tower_http::services::ServeDir;

pub(crate) const RELOAD_SCRIPT: &str =
    "<script>new EventSource('/__genbit/reload').onmessage=()=>location.reload()</script>";

pub(crate) async fn run(root: PathBuf, address: SocketAddr) -> Result<()> {
    let count = crate::build::run_dev(&root, RELOAD_SCRIPT)?;
    println!("Built {count} pages into dist/");

    let (change_tx, change_rx) = mpsc::unbounded_channel();
    let watched_root = root.clone();
    let mut watcher = notify::recommended_watcher(move |result| {
        if let Err(error) = &result {
            eprintln!("file watcher: {error}");
        }
        if let Ok(event) = result
            && relevant_change(&watched_root, &event)
        {
            let _ = change_tx.send(());
        }
    })
    .context("cannot start file watcher")?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("cannot watch {}", root.display()))?;

    let (reload_tx, _) = broadcast::channel(16);
    let app = router(&root, reload_tx.clone());
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot bind {address}"))?;
    let actual = listener
        .local_addr()
        .context("cannot read server address")?;
    println!("Server running at http://{actual}");

    let rebuild = rebuild_on_changes(root, change_rx, reload_tx);
    tokio::select! {
        result = axum::serve(listener, app) => result.context("development server failed")?,
        () = rebuild => {}
    }
    Ok(())
}

fn router(root: &Path, reload_tx: broadcast::Sender<()>) -> Router {
    let dist = root.join("dist");
    Router::new()
        .route("/__genbit/reload", get(events))
        .fallback_service(ServeDir::new(&dist))
        .layer(middleware::from_fn_with_state(dist, serve_clean_url))
        .with_state(reload_tx)
}

async fn serve_clean_url(
    State(dist): State<PathBuf>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !is_reserved_url(path)
        && let Some(route) = Route::from_request_path(path)
        && dist.join(route.output()).is_file()
    {
        let suffix = request
            .uri()
            .query()
            .map_or_else(String::new, |query| format!("?{query}"));
        if let Ok(uri) = format!("{}.html{suffix}", route.url()).parse() {
            *request.uri_mut() = uri;
        }
    }
    next.run(request).await
}

async fn events(
    State(reload_tx): State<broadcast::Sender<()>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(reload_tx.subscribe())
        .filter_map(|item| item.ok().map(|()| Ok(Event::default().data("reload"))));
    Sse::new(stream)
}

fn relevant_change(root: &Path, event: &notify::Event) -> bool {
    !matches!(event.kind, EventKind::Access(_))
        && event.paths.iter().any(|path| {
            path == &root.join("config.toml")
                || ["content", "templates", "styles", "static"]
                    .iter()
                    .any(|directory| path.starts_with(root.join(directory)))
        })
}

async fn rebuild_on_changes(
    root: PathBuf,
    mut change_rx: mpsc::UnboundedReceiver<()>,
    reload_tx: broadcast::Sender<()>,
) {
    while change_rx.recv().await.is_some() {
        tokio::time::sleep(Duration::from_millis(100)).await;
        while change_rx.try_recv().is_ok() {}
        let site = root.clone();
        match tokio::task::spawn_blocking(move || crate::build::run_dev(&site, RELOAD_SCRIPT)).await
        {
            Ok(Ok(count)) => {
                println!("Rebuilt {count} pages");
                let _ = reload_tx.send(());
            }
            Ok(Err(error)) => eprintln!("Rebuild failed: {error:#}"),
            Err(error) => eprintln!("Rebuild task failed: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::relevant_change;
    use notify::{Event, EventKind};
    use std::path::Path;

    #[test]
    fn only_source_changes_trigger_rebuild() {
        let root = Path::new("/site");
        for path in [
            "content/post.md",
            "templates/base.html",
            "styles/main.css",
            "static/a.png",
            "config.toml",
        ] {
            let event = Event::new(EventKind::Any).add_path(root.join(path));
            assert!(relevant_change(root, &event), "{path}");
        }
        for path in [
            "dist/index.html",
            ".genbit-build-123/index.html",
            "README.md",
        ] {
            let event = Event::new(EventKind::Any).add_path(root.join(path));
            assert!(!relevant_change(root, &event), "{path}");
        }
    }
}
