use std::env;
use std::path::Path;
use std::sync::mpsc::channel;
use std::time::{Instant, Duration};
use std::thread;

use chrono::prelude::*;
use iron::{Iron, Request, IronResult, Response, status};
use mount::Mount;
use staticfile::Static;
use notify::{Watcher, RecursiveMode, watcher};
use ws::{WebSocket, Sender};
use gutenberg::Site;
use gutenberg::errors::{Result};


use ::{report_elapsed_time, unravel_errors};
use console;


#[derive(Debug, PartialEq)]
enum ChangeKind {
    Content,
    Templates,
    StaticFiles,
}

const LIVE_RELOAD: &'static str = include_str!("livereload.js");


fn livereload_handler(_: &mut Request) -> IronResult<Response> {
    Ok(Response::with((status::Ok, LIVE_RELOAD.to_string())))
}


fn rebuild_done_handling(broadcaster: &Sender, res: Result<()>, reload_path: &str) {
    match res {
        Ok(_) => {
            broadcaster.send(format!(r#"
                {{
                    "command": "reload",
                    "path": "{}",
                    "originalPath": "",
                    "liveCSS": true,
                    "liveImg": true,
                    "protocol": ["http://livereload.com/protocols/official-7"]
                }}"#, reload_path)
            ).unwrap();
        },
        Err(e) => unravel_errors("Failed to build the site", &e, false)
    }
}


// Most of it taken from mdbook
pub fn serve(interface: &str, port: &str, config_file: &str) -> Result<()> {
    let start = Instant::now();
    let mut site = Site::new(env::current_dir().unwrap(), config_file)?;

    let address = format!("{}:{}", interface, port);
    // Override the base url so links work in localhost
    site.config.base_url = if site.config.base_url.ends_with('/') {
        format!("http://{}/", address)
    } else {
        format!("http://{}", address)
    };

    site.load()?;
    site.enable_live_reload();
    println!("-> Creating {} pages and {} sections", site.pages.len(), site.sections.len());
    site.build()?;
    report_elapsed_time(start);

    let ws_address = format!("{}:{}", interface, "1112");

    // Start a webserver that serves the `public` directory
    let mut mount = Mount::new();
    mount.mount("/", Static::new(Path::new("public/")));
    mount.mount("/livereload.js", livereload_handler);
    // Starts with a _ to not trigger the unused lint
    // we need to assign to a variable otherwise it will block
    let _iron = Iron::new(mount).http(address.as_str()).unwrap();

    // The websocket for livereload
    let ws_server = WebSocket::new(|_| {
        |_| {
            Ok(())
        }
    }).unwrap();
    let broadcaster = ws_server.broadcaster();
    thread::spawn(move || {
        ws_server.listen(&*ws_address).unwrap();
    });

    // And finally watching/reacting on file changes
    let (tx, rx) = channel();
    let mut watcher = watcher(tx, Duration::from_secs(2)).unwrap();
    watcher.watch("content/", RecursiveMode::Recursive).unwrap();
    watcher.watch("static/", RecursiveMode::Recursive).unwrap();
    watcher.watch("templates/", RecursiveMode::Recursive).unwrap();
    let pwd = format!("{}", env::current_dir().unwrap().display());

    println!("Listening for changes in {}/{{content, static, templates}}", pwd);
    println!("Web server is available at http://{}", address);
    println!("Press Ctrl+C to stop\n");

    use notify::DebouncedEvent::*;

    loop {
        // See https://github.com/spf13/hugo/blob/master/commands/hugo.go
        // for a more complete version of that
        match rx.recv() {
            Ok(event) => {
                match event {
                    Create(path) |
                    Write(path) |
                    Remove(path) |
                    Rename(_, path) => {
                        if is_temp_file(&path) {
                            continue;
                        }

                        println!("Change detected @ {}", Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
                        let start = Instant::now();
                        match detect_change_kind(&pwd, &path) {
                            (ChangeKind::Content, _) => {
                                console::info(&format!("-> Content changed {}", path.display()));
                                // Force refresh
                                rebuild_done_handling(&broadcaster, site.rebuild_after_content_change(&path), "/x.js");
                            },
                            (ChangeKind::Templates, _) => {
                                console::info(&format!("-> Template changed {}", path.display()));
                                // Force refresh
                                rebuild_done_handling(&broadcaster, site.rebuild_after_template_change(), "/x.js");
                            },
                            (ChangeKind::StaticFiles, p) => {
                                if path.is_file() {
                                    console::info(&format!("-> Static file changes detected {}", path.display()));
                                    rebuild_done_handling(&broadcaster, site.copy_static_file(&path), &p);
                                }
                            },
                        };
                        report_elapsed_time(start);
                    }
                    _ => {}
                }
            },
            Err(e) => console::error(&format!("Watch error: {:?}", e)),
        };
    }
}


/// Returns whether the path we received corresponds to a temp file created
/// by an editor or the OS
fn is_temp_file(path: &Path) -> bool {
    let ext = path.extension();
    match ext {
        Some(ex) => match ex.to_str().unwrap() {
            "swp" | "swx" | "tmp" | ".DS_STORE" => true,
            // jetbrains IDE
            x if x.ends_with("jb_old___") => true,
            x if x.ends_with("jb_tmp___") => true,
            x if x.ends_with("jb_bak___") => true,
            // vim
            x if x.ends_with('~') => true,
            _ => {
                if let Some(filename) = path.file_stem() {
                    // emacs
                    filename.to_str().unwrap().starts_with('#')
                } else {
                    false
                }
            }
        },
        None => {
            path.ends_with(".DS_STORE")
        },
    }
}


/// Detect what changed from the given path so we have an idea what needs
/// to be reloaded
fn detect_change_kind(pwd: &str, path: &Path) -> (ChangeKind, String) {
    let path_str = format!("{}", path.display())
        .replace(pwd, "")
        .replace("\\", "/");
    let change_kind = if path_str.starts_with("/templates") {
        ChangeKind::Templates
    } else if path_str.starts_with("/content") {
        ChangeKind::Content
    } else if path_str.starts_with("/static") {
        ChangeKind::StaticFiles
    } else {
        unreachable!("Got a change in an unexpected path: {}", path_str);
    };

    (change_kind, path_str)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_temp_file, detect_change_kind, ChangeKind};

    #[test]
    fn test_can_recognize_temp_files() {
        let testcases = vec![
            Path::new("hello.swp"),
            Path::new("hello.swx"),
            Path::new(".DS_STORE"),
            Path::new("hello.tmp"),
            Path::new("hello.html.__jb_old___"),
            Path::new("hello.html.__jb_tmp___"),
            Path::new("hello.html.__jb_bak___"),
            Path::new("hello.html~"),
            Path::new("#hello.html"),
        ];

        for t in testcases {
            assert!(is_temp_file(&t));
        }
    }

    #[test]
    fn test_can_detect_kind_of_changes() {
        let testcases = vec![
            (
                (ChangeKind::Templates, "/templates/hello.html".to_string()),
                "/home/vincent/site", Path::new("/home/vincent/site/templates/hello.html")
            ),
            (
                (ChangeKind::StaticFiles, "/static/site.css".to_string()),
                "/home/vincent/site", Path::new("/home/vincent/site/static/site.css")
            ),
            (
                (ChangeKind::Content, "/content/posts/hello.md".to_string()),
                "/home/vincent/site", Path::new("/home/vincent/site/content/posts/hello.md")
            ),
        ];

        for (expected, pwd, path) in testcases {
            assert_eq!(expected, detect_change_kind(&pwd, &path));
        }
    }


}
