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
    future::Future,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tower_http::services::{ServeDir, ServeFile};

pub(crate) const RELOAD_SCRIPT: &str =
    "<script>new EventSource('/__genbit/reload').onmessage=()=>location.reload()</script>";
const QUIET_PERIOD: Duration = Duration::from_millis(100);
const MAX_WAIT: Duration = Duration::from_millis(500);

pub(crate) async fn run(root: PathBuf, address: SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot bind {address}"))?;

    let (change_tx, change_rx) = mpsc::channel(1);
    let watched_root = root.clone();
    let mut watcher = notify::recommended_watcher(move |result| {
        if let Err(error) = &result {
            eprintln!("file watcher: {error}");
        }
        if let Ok(event) = result
            && relevant_change(&watched_root, &event)
        {
            let _ = change_tx.try_send(());
        }
    })
    .context("cannot start file watcher")?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("cannot watch {}", root.display()))?;

    // 停止のシグナルは初回ビルドの前から受け付け、配信中も同じものを待つ。シグナルでは待機を
    // やめて正常終了する。実行中のビルドはランタイムの破棄時に完了を待つため、`dist/` の
    // 切り替え途中では終わらない。
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    let count = tokio::select! {
        result = build_site(root.clone()) => result?,
        result = &mut shutdown => return stopped(result),
    };
    println!("Built {count} pages into dist/");

    let (reload_tx, _) = broadcast::channel(16);
    let app = router(&root, reload_tx.clone());
    let actual = listener
        .local_addr()
        .context("cannot read server address")?;
    println!("Server running at http://{actual}");

    let rebuild = rebuild_on_changes(change_rx, reload_tx, move || build_site(root.clone()));
    tokio::select! {
        result = axum::serve(listener, app) => result.context("development server failed"),
        () = rebuild => Ok(()),
        result = &mut shutdown => stopped(result),
    }
}

fn stopped(signal: Result<()>) -> Result<()> {
    signal?;
    println!("Stopping server");
    Ok(())
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.context("cannot listen for Ctrl+C"),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .context("cannot listen for Ctrl+C")
}

async fn build_site(root: PathBuf) -> Result<usize> {
    tokio::task::spawn_blocking(move || crate::build::run_dev(&root, RELOAD_SCRIPT))
        .await
        .context("build task failed")?
}

fn router(root: &Path, reload_tx: broadcast::Sender<()>) -> Router {
    let dist = root.join("dist");
    Router::new()
        .route("/__genbit/reload", get(events))
        .fallback_service(
            ServeDir::new(&dist).not_found_service(ServeFile::new(dist.join("404.html"))),
        )
        .layer(middleware::from_fn_with_state(dist, serve_clean_url))
        .with_state(reload_tx)
}

async fn serve_clean_url(
    State(dist): State<PathBuf>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if let Some(route) = Route::from_request_path(path)
        && !is_reserved_url(route.url())
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

async fn rebuild_on_changes<F, Fut>(
    mut change_rx: mpsc::Receiver<()>,
    reload_tx: broadcast::Sender<()>,
    mut build: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<usize>>,
{
    while wait_for_change(&mut change_rx).await {
        match build().await {
            Ok(count) => {
                println!("Rebuilt {count} pages");
                let _ = reload_tx.send(());
            }
            Err(error) => eprintln!("Rebuild failed: {error:#}"),
        }
    }
}

async fn wait_for_change(change_rx: &mut mpsc::Receiver<()>) -> bool {
    if change_rx.recv().await.is_none() {
        return false;
    }
    let deadline = Instant::now() + MAX_WAIT;
    loop {
        // Check the deadline first so a full channel cannot postpone the build indefinitely.
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => return true,
            change = change_rx.recv() => {
                if change.is_none() {
                    return true;
                }
            }
            () = tokio::time::sleep(QUIET_PERIOD) => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_WAIT, rebuild_on_changes, relevant_change, wait_for_change};
    use anyhow::{Context, Result, bail};
    use notify::{Event, EventKind};
    use std::{
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use tokio::{
        sync::{Semaphore, broadcast, mpsc},
        time::timeout,
    };

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

    #[tokio::test]
    async fn continuous_notifications_cannot_delay_rebuild_forever() -> Result<()> {
        let (change_tx, mut change_rx) = mpsc::channel(1);
        let sender = tokio::spawn(async move {
            loop {
                let _ = change_tx.try_send(());
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let started = tokio::time::Instant::now();
        let changed = timeout(Duration::from_secs(2), wait_for_change(&mut change_rx)).await?;
        sender.abort();
        assert!(changed);
        assert!(started.elapsed() >= MAX_WAIT);
        Ok(())
    }

    #[tokio::test]
    async fn notification_queued_before_rebuild_loop_is_processed() -> Result<()> {
        let (change_tx, change_rx) = mpsc::channel(1);
        change_tx.try_send(())?;
        assert!(change_tx.try_send(()).is_err());
        let (reload_tx, mut reload_rx) = broadcast::channel(1);
        let attempts = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(rebuild_on_changes(change_rx, reload_tx, {
            let attempts = Arc::clone(&attempts);
            move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async { Ok(1) }
            }
        }));
        drop(change_tx);
        timeout(Duration::from_secs(2), reload_rx.recv()).await??;
        timeout(Duration::from_secs(2), task).await??;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn changes_during_a_build_are_rebuilt_after_it_finishes() -> Result<()> {
        let (change_tx, change_rx) = mpsc::channel(1);
        let (reload_tx, mut reload_rx) = broadcast::channel(4);
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let gate = Arc::new(Semaphore::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let source_version = Arc::new(AtomicUsize::new(1));
        let task = tokio::spawn(rebuild_on_changes(change_rx, reload_tx, {
            let gate = Arc::clone(&gate);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let source_version = Arc::clone(&source_version);
            move || {
                let gate = Arc::clone(&gate);
                let active = Arc::clone(&active);
                let max_active = Arc::clone(&max_active);
                let source_version = Arc::clone(&source_version);
                let started_tx = started_tx.clone();
                async move {
                    let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(running, Ordering::SeqCst);
                    started_tx
                        .send(source_version.load(Ordering::SeqCst))
                        .context("build start receiver closed")?;
                    let permit = gate.acquire().await?;
                    permit.forget();
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(1)
                }
            }
        }));

        change_tx.try_send(())?;
        assert_eq!(
            timeout(Duration::from_secs(2), started_rx.recv())
                .await?
                .context("first build did not start")?,
            1
        );
        source_version.store(2, Ordering::SeqCst);
        change_tx.try_send(())?;
        source_version.store(3, Ordering::SeqCst);
        assert!(change_tx.try_send(()).is_err());
        assert!(started_rx.try_recv().is_err());
        gate.add_permits(1);
        timeout(Duration::from_secs(2), reload_rx.recv()).await??;
        assert_eq!(
            timeout(Duration::from_secs(2), started_rx.recv())
                .await?
                .context("follow-up build did not start")?,
            3
        );
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        gate.add_permits(1);
        timeout(Duration::from_secs(2), reload_rx.recv()).await??;
        drop(change_tx);
        timeout(Duration::from_secs(2), task).await??;
        assert_eq!(active.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn failed_build_does_not_reload_but_later_success_does() -> Result<()> {
        let (change_tx, change_rx) = mpsc::channel(1);
        let (reload_tx, mut reload_rx) = broadcast::channel(4);
        let (finished_tx, mut finished_rx) = mpsc::unbounded_channel();
        let attempts = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(rebuild_on_changes(change_rx, reload_tx, {
            let attempts = Arc::clone(&attempts);
            move || {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                let finished_tx = finished_tx.clone();
                async move {
                    finished_tx
                        .send(())
                        .context("build result receiver closed")?;
                    if attempt == 0 {
                        bail!("invalid input");
                    }
                    Ok(1)
                }
            }
        }));

        change_tx.try_send(())?;
        timeout(Duration::from_secs(2), finished_rx.recv())
            .await?
            .context("failed build did not finish")?;
        assert!(reload_rx.try_recv().is_err());
        change_tx.try_send(())?;
        timeout(Duration::from_secs(2), reload_rx.recv()).await??;
        drop(change_tx);
        timeout(Duration::from_secs(2), task).await??;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        Ok(())
    }
}
