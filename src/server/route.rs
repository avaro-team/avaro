use crate::core::ledger::Ledger;
use crate::server::model::LedgerSchema;
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::body::{boxed, Full};
use axum::extract::{Extension, Path};
use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};

use rust_embed::RustEmbed;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn graphql_playground() -> impl IntoResponse {
    Html(playground_source(GraphQLPlaygroundConfig::new("/graphql")))
}

pub async fn graphql_handler(schema: Extension<LedgerSchema>, req: GraphQLRequest) -> GraphQLResponse {
    schema.execute(req.0).await.into()
}

pub async fn file_preview(ledger: Extension<Arc<RwLock<Ledger>>>, Path(params): Path<(String,)>) -> impl IntoResponse {
    let filename = String::from_utf8(base64::decode(params.0).unwrap()).unwrap();
    let ledger = ledger.0.read().await;
    let entry = &ledger.entry.0;
    let full_path = entry.join(filename);
    if !full_path.exists() {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(boxed(Full::from("404")))
            .unwrap();
    }
    if !full_path.canonicalize().unwrap().starts_with(entry.to_str().unwrap()) {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(boxed(Full::from("404")))
            .unwrap();
    }

    let content = tokio::fs::read(&full_path).await.unwrap();
    let body = boxed(Full::from(content));
    let mime = mime_guess::from_path(&full_path).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(body)
        .unwrap()
}

pub async fn serve_frontend(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/').to_string();
    let buf = PathBuf::from_str(&path).unwrap();
    if buf.extension().is_some() {
        StaticFile(path)
    } else {
        StaticFile("index.html".to_string())
    }
}


pub async fn get_account_list(ledger: Extension<Arc<RwLock<Ledger>>>) -> impl IntoResponse {
    let ledger = ledger.read().await;
    unimplemented!()
}


#[derive(RustEmbed)]
#[folder = "frontend/build"]
struct Asset;

pub struct StaticFile<T>(pub T);

impl<T> IntoResponse for StaticFile<T>
where
    T: Into<String>,
{
    fn into_response(self) -> Response {
        let path = self.0.into();
        match Asset::get(path.as_str()) {
            Some(content) => {
                let body = boxed(Full::from(content.data));
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                Response::builder()
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .body(body)
                    .unwrap()
            }
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                // .body(())
                .body(boxed(Full::from("404")))
                .unwrap(),
        }
    }
}
