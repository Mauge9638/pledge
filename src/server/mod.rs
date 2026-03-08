use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use axum::routing::{get, post};
use axum_server::tls_rustls::RustlsConfig;
use std::io;
use tokio::task::JoinHandle;

use crate::AppState;
use crate::config::ServerConfig;
use crate::handlers::{health::health_handler, metrics::metrics_handler, query::query_handler};
pub mod state;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/query", post(query_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

pub async fn run_server(server_config: &ServerConfig, state: AppState) {
    let routes = create_router(state);
    let cloned_routes = routes.clone();
    let port = server_config.port;

    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{}", 1337)).await {
            Ok(listener) => {
                println!("Server listening on HTTP on port {}", 1337);
                listener
            }
            Err(err) => {
                eprintln!("Failed to bind to port {}: {}", 1337, err);
                return;
            }
        };
        loop {
            match listener.accept().await {
                Ok((stream, ip)) => tokio::spawn(async move {
                    println!(
                        "Accepted connection from {:?} on ip {:?}",
                        stream.peer_addr(),
                        ip
                    );
                    loop {
                        let _ = stream.readable().await;
                        let mut buffer = [0u8; 1024];

                        match stream.try_read(&mut buffer) {
                            Ok(0) => break,
                            Ok(n) => {
                                println!("read {} bytes", n);
                                println!("raw content {:?}", &buffer[..n]);
                                println!("As string {:?}", std::str::from_utf8(&buffer[..n]));
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                continue;
                            }
                            Err(e) => {
                                eprintln!("Error occured: {}", e);
                                return;
                            }
                        }
                        let response = b"N";
                        match stream.try_write(response) {
                            Ok(0) => break,
                            Ok(n) => {
                                println!("sent {} bytes", n);
                                println!("bytes sent: {:?}", &response[..n]);
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                continue;
                            }
                            Err(e) => {
                                eprintln!("Error occured: {}", e);
                                return;
                            }
                        }
                    }
                }),
                Err(err) => tokio::spawn(async move {
                    eprintln!("Failed to accept connection: {}", err);
                }),
            };
        }
    });

    let http_handle = tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await {
            Ok(listener) => {
                println!("Server listening on HTTP on port {}", port);
                listener
            }
            Err(err) => {
                eprintln!("Failed to bind to port {}: {}", port, err);
                return;
            }
        };

        match axum::serve(listener, routes).await {
            Ok(_) => {}
            Err(err) => {
                eprintln!("Failed to serve HTTP server: {}", err);
            }
        };
    });

    let https_handle: Option<JoinHandle<()>> = if let (Some(https_port), Some(cert), Some(key)) = (
        server_config.https_port,
        server_config.tls_cert_path.clone(),
        server_config.tls_key_path.clone(),
    ) && https_port != port
    {
        Some(tokio::spawn(async move {
            let config =
                match RustlsConfig::from_pem_file(PathBuf::from(cert), PathBuf::from(key)).await {
                    Ok(config) => {
                        println!("Server listening on HTTPS on port {}", https_port);
                        config
                    }
                    Err(err) => {
                        eprintln!("Failed to load TLS certificate and key: {}", err);
                        return;
                    }
                };

            let addr = SocketAddr::from(([0, 0, 0, 0], https_port));
            match axum_server::bind_rustls(addr, config)
                .serve(cloned_routes.into_make_service())
                .await
            {
                Ok(_) => {}
                Err(err) => eprintln!("Failed to start HTTPS server: {}", err),
            };
        }))
    } else {
        None
    };

    match https_handle {
        Some(https_handle) => {
            let (http, https) = tokio::join!(http_handle, https_handle);
            match http {
                Ok(_) => {}
                Err(err) => eprintln!("{}", err),
            };
            match https {
                Ok(_) => {}
                Err(err) => eprintln!("{}", err),
            };
        }
        None => {
            let _ = http_handle.await;
        }
    }
}
