//! Shared household font actions, used by HTTP and MCP. Only workers parse font programs.
use crate::{
    http::{App, Caller},
    people::Principal,
    service::ApiError,
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use base64::Engine;
use cutcraft_web_protocol::worker::{WorkerReply, WorkerRequest};
use serde::Deserialize;
use serde_json::{Value, json};
fn person(caller: &Caller) -> Result<Principal, ApiError> {
    Principal::of(&caller.identity).ok_or_else(|| ApiError::new(401, "login", "Sign in first."))
}
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(422, "font", message)
}
pub async fn list(app: &App, p: &Principal) -> Result<Value, ApiError> {
    match app.design.ask(p, WorkerRequest::Fonts).await? {
        WorkerReply::Fonts { value } => Ok(value),
        WorkerReply::Failed { message } => Err(invalid(message)),
        _ => Err(super::unexpected()),
    }
}
pub async fn install(
    app: &App,
    p: &Principal,
    name: String,
    bytes: Vec<u8>,
    replace: Option<String>,
) -> Result<Value, ApiError> {
    if bytes.is_empty() || bytes.len() > cameo_pi_core::fonts::MAX_FONT_BYTES {
        return Err(invalid("Fonts must be between 1 byte and 16 MiB."));
    }
    cameo_pi_core::library::check_name(&name).map_err(|e| invalid(e.to_string()))?;
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let faces = match app
        .design
        .ask(p, WorkerRequest::FontInspect { data_base64 })
        .await?
    {
        WorkerReply::Fonts { value } => {
            serde_json::from_value(value).map_err(|_| invalid("Invalid font inspection result."))?
        }
        WorkerReply::Failed { message } => return Err(invalid(message)),
        _ => return Err(super::unexpected()),
    };
    let font = app
        .service
        .library
        .install_font(name, &bytes, faces, replace.as_deref())
        .map_err(invalid)?;
    Ok(json!({"font":font}))
}
pub fn remove(app: &App, id: &str) -> Result<Value, ApiError> {
    app.service.library.remove_font(id).map_err(invalid)?;
    Ok(json!({"removed":id}))
}
pub async fn download(app: &App, p: &Principal, id: String) -> Result<(String, Vec<u8>), ApiError> {
    if let Some(id) = id.strip_prefix("library-") {
        let archive = app.service.library.fonts().map_err(invalid)?;
        let font = archive
            .fonts
            .iter()
            .find(|font| font.id == id)
            .ok_or_else(|| invalid("Font not found."))?;
        return Ok((
            font.name.clone(),
            app.service.library.font_bytes(id).map_err(invalid)?,
        ));
    }
    match app
        .design
        .ask(p, WorkerRequest::FontDownload { id })
        .await?
    {
        WorkerReply::Original {
            filename,
            data_base64,
        } => Ok((
            filename,
            base64::engine::general_purpose::STANDARD
                .decode(data_base64)
                .map_err(|_| invalid("Invalid font download."))?,
        )),
        WorkerReply::Failed { message } => Err(invalid(message)),
        _ => Err(super::unexpected()),
    }
}
pub async fn get(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
) -> Result<Response, ApiError> {
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(list(&app, &person(&caller)?).await?),
    )
        .into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    name: String,
    replace: Option<String>,
}
pub async fn upload(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Query(query): Query<Upload>,
    bytes: Bytes,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        install(
            &app,
            &person(&caller)?,
            query.name,
            bytes.to_vec(),
            query.replace,
        )
        .await?,
    ))
}
pub async fn delete(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    person(&caller)?;
    Ok(Json(remove(&app, &id)?))
}
pub async fn get_download(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let (name, bytes) = download(&app, &person(&caller)?, id).await?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .-_()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok((
        [
            (header::CACHE_CONTROL, "no-store".into()),
            (header::CONTENT_TYPE, "application/octet-stream".into()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{safe}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// Restore validation gets its own bounded worker; the live design sessions remain untouched.
pub(crate) async fn validate_restored(
    dir: &std::path::Path,
    lists: &std::path::Path,
    scratch: &std::path::Path,
) -> Result<(), String> {
    let archive = cameo_pi_core::fonts::inspect_archive(dir)?;
    if archive.fonts.is_empty() {
        return Ok(());
    }
    let area = scratch.join("font-validator");
    std::fs::create_dir_all(area.join("drafts")).map_err(|e| e.to_string())?;
    #[cfg(not(test))]
    let mode = super::Mode::Process(std::env::current_exe().map_err(|e| e.to_string())?);
    #[cfg(test)]
    let mode = super::Mode::Thread;
    let mut link = super::Link::start(&mode, &area, lists)?;
    let result = async {
        for font in archive.fonts {
            let bytes =
                std::fs::read(dir.join(format!("{}.ttf", font.id))).map_err(|e| e.to_string())?;
            let request = WorkerRequest::FontInspect {
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            };
            let reply =
                tokio::time::timeout(std::time::Duration::from_secs(30), link.ask(&request))
                    .await
                    .map_err(|_| "Font validation timed out.")??;
            match reply {
                WorkerReply::Fonts { value } => {
                    let actual: Vec<cameo_pi_core::fonts::Face> =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    if actual != font.faces {
                        return Err(
                            "Restored font metadata does not match its font program.".into()
                        );
                    }
                }
                WorkerReply::Failed { message } => return Err(message),
                _ => return Err("Invalid font validation reply.".into()),
            }
        }
        Ok(())
    }
    .await;
    link.close().await;
    let _ = std::fs::remove_dir_all(area);
    result
}
