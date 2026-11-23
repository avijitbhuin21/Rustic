//! `rustic-film://<session>/<path>`: desktop adapter over `rustic_app::film_preview`.
//! The session id is the URL host, so a film page's root-absolute URLs stay inside its session.

use rustic_app::film_preview::{self, FilmResponse, Layout};
use tauri::http::{Method, Request, Response};

/// Serve one film-session request.
pub fn handle(req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let resp = if req.method() != Method::GET {
        FilmResponse::error(405, "Only GET is served.")
    } else {
        match film_preview::percent_decode(req.uri().path()) {
            Some(rel) => film_preview::sessions().respond(
                req.uri().host().unwrap_or(""),
                &rel,
                Layout::Scheme,
            ),
            None => FilmResponse::error(400, "Bad path encoding."),
        }
    };
    let mut out = Response::builder().status(resp.status);
    for (name, value) in resp.headers() {
        out = out.header(name, value);
    }
    out.body(resp.body).unwrap_or_default()
}
