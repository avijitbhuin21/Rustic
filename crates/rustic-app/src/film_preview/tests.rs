use std::fs;
use std::path::{Path, PathBuf};

use super::serve::rewrite_for_test;
use super::{FilmSessions, Layout};

/// A throwaway project: `lib/` is a package root holding a film two folders down.
fn project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustic-film-test-{}", uuid::Uuid::new_v4()));
    let film = dir.join("lib/films/demo");
    fs::create_dir_all(&film).unwrap();
    fs::create_dir_all(dir.join("lib/core")).unwrap();
    fs::create_dir_all(dir.join("lib/.git")).unwrap();
    fs::create_dir_all(dir.join("lib/films/demo/out")).unwrap();
    fs::write(dir.join("lib/package.json"), "{}").unwrap();
    fs::write(dir.join("lib/core/lib.js"), "export const a = 1;").unwrap();
    fs::write(dir.join("lib/.env"), "SECRET=1").unwrap();
    fs::write(dir.join("lib/.git/config"), "x").unwrap();
    fs::write(dir.join("secret.txt"), "outside the served folder").unwrap();
    fs::write(film.join("out/mix.wav"), "RIFF").unwrap();
    fs::write(
        film.join("main.js"),
        "import { a } from '/core/lib.js'; fetch('/other/x'); import('//cdn.example/x.js');",
    )
    .unwrap();
    fs::write(
        film.join("index.html"),
        "<!doctype html><html><head><title>t</title></head><body><script type=\"module\" src=\"main.js\"></script></body></html>",
    )
    .unwrap();
    fs::canonicalize(&dir).unwrap()
}

fn open(s: &FilmSessions, dir: &Path) -> super::FilmOpen {
    s.open(
        &dir.join("lib/films/demo/index.html"),
        &[dir.to_path_buf()],
        &[],
        false,
    )
    .unwrap()
}

fn body(r: &super::FilmResponse) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

const WEB: Layout<'static> = Layout::Path {
    host: "rustic.test:8080",
};

#[test]
fn serves_from_the_package_root_and_finds_the_mix() {
    let dir = project();
    let s = FilmSessions::default();
    let o = open(&s, &dir);
    assert_eq!(PathBuf::from(&o.root), dir.join("lib"));
    assert_eq!(PathBuf::from(&o.project), dir);
    assert_eq!(o.url, format!("/film/{}/@rustic/shell", o.id));
    assert!(o.audio.as_deref().is_some_and(|a| a.ends_with("mix.wav")));

    let shell = s.respond(&o.id, "@rustic/shell", WEB);
    assert_eq!(shell.status, 200);
    assert!(body(&shell).contains(&format!("src=\"/film/{}/films/demo/index.html\"", o.id)));
    assert!(shell
        .csp
        .contains(&format!("frame-src rustic.test:8080/film/{}/;", o.id)));
    assert!(shell.csp.contains("sandbox allow-scripts"));

    let lib = s.respond(&o.id, "core/lib.js", WEB);
    assert_eq!(lib.status, 200);
    assert_eq!(lib.content_type, "text/javascript; charset=utf-8");
    assert!(lib.csp.contains("default-src 'none'"));
    assert!(lib
        .csp
        .contains(&format!("connect-src rustic.test:8080/film/{}/ ", o.id)));
    assert!(lib.csp.contains("sandbox allow-scripts"));
    assert!(lib
        .headers()
        .iter()
        .any(|(k, v)| *k == "access-control-allow-origin" && v == "*"));
    fs::remove_dir_all(dir).ok();
}

#[test]
fn injects_the_bridge_and_repoints_root_absolute_urls_in_path_layout_only() {
    let dir = project();
    let s = FilmSessions::default();
    let o = open(&s, &dir);
    let page = body(&s.respond(&o.id, "films/demo/index.html", WEB));
    assert!(page.starts_with(&format!(
        "<!doctype html><script src=\"/film/{}/@rustic/bridge.js\"></script><html>",
        o.id
    )));
    let js = body(&s.respond(&o.id, "films/demo/main.js", WEB));
    assert!(js.contains(&format!("from '/film/{}/core/lib.js'", o.id)));
    assert!(
        js.contains("fetch('/other/x')"),
        "names that are not in the folder stay untouched"
    );
    assert!(js.contains("import('//cdn.example/x.js')"));

    let page = body(&s.respond(&o.id, "films/demo/index.html", Layout::Scheme));
    assert!(page.starts_with("<!doctype html><script src=\"/@rustic/bridge.js\"></script><html>"));
    let js = s.respond(&o.id, "films/demo/main.js", Layout::Scheme);
    assert!(body(&js).contains("from '/core/lib.js'"));
    assert!(js
        .csp
        .contains(&format!("script-src rustic-film://{} ", o.id)));
    assert!(body(&s.respond(&o.id, "@rustic/bridge.js", WEB)).contains("rustic-film-host"));

    // A page cannot get a script in ahead of the bridge, and a non-HTML document gets no script at all.
    fs::write(
        dir.join("lib/core/early.html"),
        "<html><script>evil()</script><head></head></html>",
    )
    .unwrap();
    fs::write(
        dir.join("lib/core/a.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>1</script></svg>",
    )
    .unwrap();
    assert!(body(&s.respond(&o.id, "core/early.html", Layout::Scheme))
        .starts_with("<script src=\"/@rustic/bridge.js\"></script><html><script>evil()"));
    assert_eq!(
        s.respond(&o.id, "core/a.svg", WEB).csp,
        "default-src 'none'; sandbox"
    );
    fs::remove_dir_all(dir).ok();
}

#[test]
fn rewrite_only_touches_quoted_paths_and_css_urls() {
    assert_eq!(
        rewrite_for_test("a = \"/core/x\"; b = (/core/.test(s));", &["core"], false),
        "a = \"/film/ID/core/x\"; b = (/core/.test(s));"
    );
    assert_eq!(
        rewrite_for_test("src: url(/core/f.woff2)", &["core"], true),
        "src: url(/film/ID/core/f.woff2)"
    );
    assert_eq!(
        rewrite_for_test("'/corex/a' '/core'", &["core"], false),
        "'/corex/a' '/film/ID/core'"
    );
}

#[test]
fn refuses_hidden_files_traversal_and_unknown_sessions() {
    let dir = project();
    let s = FilmSessions::default();
    let o = open(&s, &dir);
    for (rel, status) in [
        (".env", 403),
        (".git/config", 403),
        ("films/../.env", 403),
        ("../secret.txt", 403),
        ("films/demo/../../../secret.txt", 403),
        ("films/demo", 404),
        ("missing.js", 404),
        ("core\\lib.js", 400),
        ("core/lib.js\0", 400),
        ("C:/x", 400),
    ] {
        let r = s.respond(&o.id, rel, WEB);
        assert_eq!(r.status, status, "{rel}");
        assert!(
            !body(&r).contains("SECRET") && !body(&r).contains("outside the served"),
            "{rel}"
        );
        assert!(r.csp.contains("sandbox"), "{rel}");
    }
    assert_eq!(s.respond("0000", "core/lib.js", WEB).status, 404);
    assert_eq!(
        s.respond(
            &o.id,
            "core/lib.js",
            Layout::Path {
                host: "evil.test; script-src *"
            }
        )
        .status,
        400
    );
    s.close(&o.id);
    assert_eq!(s.respond(&o.id, "core/lib.js", WEB).status, 404);
    fs::remove_dir_all(dir).ok();
}

#[cfg(unix)]
#[test]
fn refuses_symlinks_that_leave_the_folder_or_point_at_hidden_files() {
    let dir = project();
    std::os::unix::fs::symlink(dir.join("secret.txt"), dir.join("lib/core/escape.txt")).unwrap();
    std::os::unix::fs::symlink(dir.join("lib/.env"), dir.join("lib/core/env.txt")).unwrap();
    let s = FilmSessions::default();
    let o = open(&s, &dir);
    assert_eq!(s.respond(&o.id, "core/escape.txt", WEB).status, 403);
    assert_eq!(s.respond(&o.id, "core/env.txt", WEB).status, 403);
    fs::remove_dir_all(dir).ok();
}

#[test]
fn open_is_limited_to_html_pages_inside_a_project_and_outside_denied_folders() {
    let dir = project();
    let s = FilmSessions::default();
    let page = dir.join("lib/films/demo/index.html");
    assert!(s.open(&page, &[], &[], false).is_err(), "no project");
    assert!(
        s.open(&page, &[dir.join("lib/core")], &[], false).is_err(),
        "other project"
    );
    assert!(
        s.open(
            &dir.join("lib/core/lib.js"),
            std::slice::from_ref(&dir),
            &[],
            false
        )
        .is_err(),
        "not html"
    );
    assert!(
        s.open(&page, std::slice::from_ref(&dir), &[dir.join("lib")], false)
            .is_err(),
        "denied folder"
    );

    // A project root below the package root caps the served folder at the project.
    let o = s.open(&page, &[dir.join("lib/films")], &[], true).unwrap();
    assert_eq!(PathBuf::from(&o.root), dir.join("lib/films/demo"));
    assert_eq!(o.url, format!("rustic-film://{}/@rustic/shell", o.id));
    assert_eq!(s.respond(&o.id, "index.html", Layout::Scheme).status, 200);
    assert_eq!(
        s.respond(&o.id, "../../core/lib.js", Layout::Scheme).status,
        403
    );

    // A page in a hidden folder below the served folder is refused, not served by accident.
    fs::create_dir_all(dir.join("lib/.hidden")).unwrap();
    fs::write(dir.join("lib/.hidden/p.html"), "<html></html>").unwrap();
    assert!(s
        .open(
            &dir.join("lib/.hidden/p.html"),
            std::slice::from_ref(&dir),
            &[],
            false
        )
        .is_err());
    fs::remove_dir_all(dir).ok();
}

#[test]
fn oldest_session_is_dropped_at_the_cap() {
    let dir = project();
    let s = FilmSessions::default();
    let first = open(&s, &dir);
    let rest: Vec<_> = (0..super::MAX_SESSIONS).map(|_| open(&s, &dir)).collect();
    assert_eq!(s.respond(&first.id, "core/lib.js", WEB).status, 404);
    assert_eq!(s.respond(&rest[0].id, "core/lib.js", WEB).status, 200);
    fs::remove_dir_all(dir).ok();
}

#[test]
fn page_sessions_serve_the_project_with_the_page_bridge_and_frozen_css() {
    let dir = project();
    fs::write(
        dir.join("lib/films/demo/site.css"),
        "@media (min-height: 50vh) { .a { height: 100vh; } } .b { min-height: calc(100dvh - 2rem); top: -50vh; font: 2vmax/1 x; } .c { background: url(a-10vh.png); content: \"5vh\"; width: 10vw; } /* 3vh */ .d1vh { margin: 1.5e1vh }",
    )
    .unwrap();
    let s = FilmSessions::default();
    let o = s
        .open_page(
            &dir.join("lib/films/demo/index.html"),
            std::slice::from_ref(&dir),
            &[],
            false,
        )
        .unwrap();
    assert_eq!(
        PathBuf::from(&o.root),
        dir,
        "page sessions serve the whole project"
    );
    assert!(o.audio.is_none());

    let shell = body(&s.respond(&o.id, "@rustic/shell", WEB));
    assert!(shell.contains("background:#fff"));
    assert!(shell.contains(&format!("src=\"/film/{}/lib/films/demo/index.html\"", o.id)));

    let bridge = body(&s.respond(&o.id, "@rustic/bridge.js", WEB));
    assert!(bridge.contains("rustic-page") && !bridge.contains("rustic-film-host"));

    let css = s.respond(&o.id, "lib/films/demo/site.css", WEB);
    assert!(css.csp.contains("sandbox"));
    assert_eq!(
        body(&css),
        "@media (min-height: 50vh) { .a { height: calc(100 * var(--rustic-vh, 1vh)); } } .b { min-height: calc(calc(100 * var(--rustic-vh, 1vh)) - 2rem); top: calc(-50 * var(--rustic-vh, 1vh)); font: calc(2 * max(1vw, var(--rustic-vh, 1vh)))/1 x; } .c { background: url(a-10vh.png); content: \"5vh\"; width: 10vw; } /* 3vh */ .d1vh { margin: calc(1.5e1 * var(--rustic-vh, 1vh)) }"
    );

    let page = s.respond(&o.id, "lib/films/demo/index.html", WEB);
    assert!(
        page.csp.contains("script-src rustic.test:8080/film/") && page.csp.contains(" https: ")
    );
    assert!(page.csp.contains("connect-src") && page.csp.contains("frame-src 'none'"));
    assert!(page.csp.contains("sandbox allow-scripts"));
    assert_eq!(s.respond(&o.id, "lib/.env", WEB).status, 403);
    assert_eq!(s.respond(&o.id, "../secret.txt", WEB).status, 403);

    // Film sessions keep their closed network and leave CSS alone.
    let f = open(&s, &dir);
    let film_css = body(&s.respond(&f.id, "films/demo/site.css", WEB));
    assert!(film_css.contains(".a { height: 100vh; }"));
    assert!(!s
        .respond(&f.id, "films/demo/index.html", WEB)
        .csp
        .contains("https:"));
    fs::remove_dir_all(dir).ok();
}

#[test]
fn freezing_viewport_units_leaves_other_tokens_alone() {
    use super::serve::freeze_for_test as freeze;
    assert_eq!(
        freeze("a{h:.5svh}"),
        "a{h:calc(.5 * var(--rustic-vh, 1vh))}"
    );
    assert_eq!(freeze("a{h:2s;w:#100vh}"), "a{h:2s;w:#100vh}");
    assert_eq!(
        freeze("a{t:translateY(-100lvh)}"),
        "a{t:translateY(calc(-100 * var(--rustic-vh, 1vh)))}"
    );
    assert_eq!(
        freeze("a{w:5vmin}"),
        "a{w:calc(5 * min(1vw, var(--rustic-vh, 1vh)))}"
    );
    assert_eq!(freeze("a{w:5vhx}"), "a{w:5vhx}");
    assert_eq!(
        freeze("@supports (height: 100dvh) { a { h: 100dvh } }"),
        "@supports (height: 100dvh) { a { h: calc(100 * var(--rustic-vh, 1vh)) } }"
    );
    assert_eq!(freeze("a{c:'unterminated"), "a{c:'unterminated");
}
