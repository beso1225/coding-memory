//! Small, bounded HTTP viewer. Only fixed GET routes on IPv4 loopback are served.
use anyhow::Result;
use std::{io::Write, net::Ipv4Addr, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    time::timeout,
};

const CSP: &str = "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub async fn run(root: PathBuf, port: u16) -> Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let authority = listener.local_addr()?.to_string();
    println!("Dashboard: http://{authority}");
    std::io::stdout().flush()?;
    let capacity = Arc::new(Semaphore::new(16));
    let shutdown = async {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! { r = tokio::signal::ctrl_c() => r?, _ = terminate.recv() => {} }
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await?;
        Ok::<_, std::io::Error>(())
    };
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            signal = &mut shutdown => { signal?; return Ok(()); }
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                let Ok(permit) = capacity.clone().try_acquire_owned() else { continue };
                let root = root.clone();
                let authority = authority.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    // Client failures are neither pipeline failures nor raw diagnostics.
                    let _ = handle(socket, root, authority).await;
                });
            }
        }
    }
}

async fn handle(mut socket: TcpStream, root: PathBuf, authority: String) -> Result<()> {
    let request = timeout(Duration::from_secs(5), read_request(&mut socket)).await??;
    let (code, kind, body) = match route(&request, &authority) {
        Ok("/") => (200, "text/html", shell()),
        Ok("/dashboard.js") => (200, "text/javascript", include_str!("dashboard.js").into()),
        Ok("/snapshot") => {
            let html =
                tokio::task::spawn_blocking(move || crate::dashboard::render_live(&root)).await?;
            (200, "text/html", html)
        }
        Ok(_) => (404, "text/plain", "Not found".into()),
        Err(code) => (code, "text/plain", "Request rejected".into()),
    };
    let reason = match code {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Bad Request",
    };
    let allow = if code == 405 { "Allow: GET\r\n" } else { "" };
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {kind}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: {CSP}\r\n{allow}\r\n{body}",
        body.len()
    );
    timeout(
        Duration::from_secs(5),
        socket.write_all(response.as_bytes()),
    )
    .await??;
    Ok(())
}

async fn read_request(socket: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::new();
    let mut buf = [0; 1024];
    while bytes.len() < 8192 {
        let n = socket.read(&mut buf).await?;
        anyhow::ensure!(n > 0, "Incomplete request");
        bytes.extend_from_slice(&buf[..n]);
        if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(String::from_utf8(bytes)?);
        }
    }
    anyhow::bail!("Request too large")
}

fn route<'a>(request: &'a str, authority: &str) -> std::result::Result<&'a str, u16> {
    let mut lines = request.split("\r\n");
    let first: Vec<_> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if first.len() != 3 || first[2] != "HTTP/1.1" {
        return Err(400);
    }
    if first[0] != "GET" {
        return Err(405);
    }
    let mut hosts = 0;
    let origin = format!("http://{authority}");
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(400_u16)?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(400);
        }
        let value = value.trim();
        if name.eq_ignore_ascii_case("host") {
            hosts += 1;
            if value != authority {
                return Err(403);
            }
        }
        if name.eq_ignore_ascii_case("origin") && value != origin {
            return Err(403);
        }
        if name.eq_ignore_ascii_case("sec-fetch-site") && !matches!(value, "same-origin" | "none") {
            return Err(403);
        }
        if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-length") && value != "0")
        {
            return Err(400);
        }
    }
    if hosts != 1 {
        return Err(400);
    }
    Ok(first[1])
}

fn shell() -> String {
    let mut html = include_str!("dashboard.html").replace(
        "default-src 'none';",
        "default-src 'none'; script-src 'self'; connect-src 'self';",
    );
    html.push_str("<p id=\"refresh-status\" class=timestamp role=status aria-live=polite>Connecting to the local viewer…</p><noscript>JavaScript is required for live updates. Use the dashboard command without --serve for a static snapshot.</noscript></main><script src=\"/dashboard.js\" defer></script></body></html>");
    html
}
