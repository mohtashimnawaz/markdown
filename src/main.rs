
use std::fs;
use std::path::Path;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use actix_web::{web, App, HttpServer, HttpResponse, Result, middleware::Logger};
use actix_files::Files;
use serde::{Deserialize, Serialize};
use pulldown_cmark::{Parser, html};
use tera::{Tera, Context};
use notify::{Watcher, RecommendedWatcher, Config};
use tokio::time::sleep;

#[derive(Debug, Deserialize, Serialize)]
struct FrontMatter {
    title: String,
    date: String,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct Post {
    slug: String,
    frontmatter: FrontMatter,
    content: String,
    html: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    tag: Option<String>,
}

fn parse_post(path: &Path) -> Option<Post> {
    let raw = fs::read_to_string(path).ok()?;
    let parts: Vec<&str> = raw.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }
    let fm_str = parts[1];
    let content = parts[2].trim().to_string();
    let frontmatter: FrontMatter = serde_yaml::from_str(fm_str).ok()?;
    let parser = Parser::new(&content);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    let slug = path.file_stem()?.to_string_lossy().to_string();
    Some(Post {
        slug,
        frontmatter,
        content,
        html: html_output,
    })
}

fn load_posts() -> Vec<Post> {
    let content_dir = "content";
    let mut posts = Vec::new();
    if let Ok(entries) = fs::read_dir(content_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Some(post) = parse_post(&path) {
                    println!("Loaded post: {}", post.frontmatter.title);
                    posts.push(post);
                }
            }
        }
    }
    // Sort posts by date (newest first)
    posts.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    posts
}

fn main() {
    println!("Starting markdown blog server...");
    
    let posts = load_posts();
    if let Err(e) = start_server(posts) {
        eprintln!("Server error: {}", e);
        std::process::exit(1);
    }
}

#[actix_web::main]
async fn start_server(initial_posts: Vec<Post>) -> std::io::Result<()> {
    // Create shared post cache
    let mut post_cache = HashMap::new();
    for post in initial_posts {
        post_cache.insert(post.slug.clone(), post);
    }
    let post_cache = Arc::new(Mutex::new(post_cache));
    
    // Start file watcher in background
    let cache_for_watcher = post_cache.clone();
    tokio::spawn(async move {
        watch_files(cache_for_watcher).await;
    });
    
    // Initialize templates
    let tera = Tera::new("templates/**/*").unwrap_or_else(|_| {
        println!("Warning: No templates found, creating empty Tera instance");
        Tera::default()
    });
    
    println!("Server starting at http://127.0.0.1:8080");
    
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(post_cache.clone()))
            .app_data(web::Data::new(tera.clone()))
            .wrap(Logger::default())
            .route("/", web::get().to(home))
            .route("/search", web::get().to(search))
            .route("/about", web::get().to(about))
            .route("/posts/{slug}", web::get().to(post_detail))
            .service(Files::new("/static", "static"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}

async fn watch_files(post_cache: Arc<Mutex<HashMap<String, Post>>>) {
    use notify::EventKind;
    
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                    let _ = tx.blocking_send(event);
                }
            }
        },
        Config::default(),
    ).unwrap();
    
    if watcher.watch(Path::new("content"), notify::RecursiveMode::NonRecursive).is_err() {
        println!("Warning: Could not watch content directory for changes");
        return;
    }
    
    println!("Watching content directory for changes...");
    
    while let Some(_event) = rx.recv().await {
        // Add a small delay to avoid processing rapid file changes
        sleep(Duration::from_millis(100)).await;
        
        println!("Content changed, reloading posts...");
        let new_posts = load_posts();
        
        let mut cache = post_cache.lock().unwrap();
        cache.clear();
        for post in new_posts {
            cache.insert(post.slug.clone(), post);
        }
        println!("Posts reloaded!");
    }
}

async fn home(
    posts: web::Data<Arc<Mutex<HashMap<String, Post>>>>,
    tera: web::Data<Tera>,
) -> Result<HttpResponse> {
    let posts_guard = posts.lock().unwrap();
    let mut context = Context::new();
    let mut posts_vec: Vec<&Post> = posts_guard.values().collect();
    posts_vec.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    context.insert("posts", &posts_vec);
    
    // Get all unique tags
    let mut all_tags = std::collections::HashSet::new();
    for post in &posts_vec {
        if let Some(tags) = &post.frontmatter.tags {
            for tag in tags {
                all_tags.insert(tag.clone());
            }
        }
    }
    let mut tags_vec: Vec<String> = all_tags.into_iter().collect();
    tags_vec.sort();
    context.insert("all_tags", &tags_vec);
    
    match tera.render("home.html", &context) {
        Ok(rendered) => Ok(HttpResponse::Ok().content_type("text/html").body(rendered)),
        Err(_) => Ok(HttpResponse::Ok().content_type("text/html").body(
            format!("<h1>Blog Posts</h1><ul>{}</ul>",
                posts_vec.iter().map(|p| format!(
                    r#"<li><a href="/posts/{}">{}</a> - {}</li>"#,
                    p.slug, p.frontmatter.title, p.frontmatter.date
                )).collect::<Vec<_>>().join("")
            )
        ))
    }
}

async fn post_detail(
    path: web::Path<String>,
    posts: web::Data<Arc<Mutex<HashMap<String, Post>>>>,
    tera: web::Data<Tera>,
) -> Result<HttpResponse> {
    let slug = path.into_inner();
    let posts_guard = posts.lock().unwrap();
    
    if let Some(post) = posts_guard.get(&slug) {
        let mut context = Context::new();
        context.insert("post", post);
        
        match tera.render("post.html", &context) {
            Ok(rendered) => Ok(HttpResponse::Ok().content_type("text/html").body(rendered)),
            Err(_) => Ok(HttpResponse::Ok().content_type("text/html").body(
                format!("<h1>{}</h1><p>Date: {}</p><div>{}</div>",
                    post.frontmatter.title,
                    post.frontmatter.date,
                    post.html
                )
            ))
        }
    } else {
        // Try to render 404 template, fall back to basic HTML
        match tera.render("404.html", &Context::new()) {
            Ok(rendered) => Ok(HttpResponse::NotFound().content_type("text/html").body(rendered)),
            Err(_) => Ok(HttpResponse::NotFound().content_type("text/html").body("<h1>404 - Post Not Found</h1>"))
        }
    }
}

async fn about(tera: web::Data<Tera>) -> Result<HttpResponse> {
    match tera.render("about.html", &Context::new()) {
        Ok(rendered) => Ok(HttpResponse::Ok().content_type("text/html").body(rendered)),
        Err(_) => Ok(HttpResponse::Ok().content_type("text/html").body(
            "<h1>About</h1><p>This is a Rust-powered Markdown blog engine built with Actix-web!</p><a href=\"/\">← Back to Home</a>"
        ))
    }
}

async fn search(
    query: web::Query<SearchQuery>,
    posts: web::Data<Arc<Mutex<HashMap<String, Post>>>>,
    tera: web::Data<Tera>,
) -> Result<HttpResponse> {
    let posts_guard = posts.lock().unwrap();
    let mut filtered_posts: Vec<&Post> = posts_guard.values().collect();
    
    // Filter by search query
    if let Some(search_term) = &query.q {
        if !search_term.trim().is_empty() {
            let search_lower = search_term.to_lowercase();
            filtered_posts.retain(|post| {
                post.frontmatter.title.to_lowercase().contains(&search_lower) ||
                post.content.to_lowercase().contains(&search_lower)
            });
        }
    }
    
    // Filter by tag
    if let Some(tag) = &query.tag {
        if !tag.trim().is_empty() {
            filtered_posts.retain(|post| {
                post.frontmatter.tags.as_ref()
                    .map(|tags| tags.iter().any(|t| t == tag))
                    .unwrap_or(false)
            });
        }
    }
    
    // Sort by date (newest first)
    filtered_posts.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    
    let mut context = Context::new();
    context.insert("posts", &filtered_posts);
    context.insert("search_query", &query.q);
    context.insert("selected_tag", &query.tag);
    
    // Get all unique tags for the filter dropdown
    let mut all_tags = std::collections::HashSet::new();
    for post in posts_guard.values() {
        if let Some(tags) = &post.frontmatter.tags {
            for tag in tags {
                all_tags.insert(tag.clone());
            }
        }
    }
    let mut tags_vec: Vec<String> = all_tags.into_iter().collect();
    tags_vec.sort();
    context.insert("all_tags", &tags_vec);
    
    match tera.render("search.html", &context) {
        Ok(rendered) => Ok(HttpResponse::Ok().content_type("text/html").body(rendered)),
        Err(_) => {
            let results_html = if filtered_posts.is_empty() {
                "<p>No posts found.</p>".to_string()
            } else {
                format!("<ul>{}</ul>", 
                    filtered_posts.iter().map(|p| format!(
                        r#"<li><a href="/posts/{}">{}</a> - {}</li>"#,
                        p.slug, p.frontmatter.title, p.frontmatter.date
                    )).collect::<Vec<_>>().join("")
                )
            };
            Ok(HttpResponse::Ok().content_type("text/html").body(
                format!("<h1>Search Results</h1>{}<a href=\"/\">← Back to Home</a>", results_html)
            ))
        }
    }
}
