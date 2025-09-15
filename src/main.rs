
use std::fs;
use std::path::Path;
use std::collections::HashMap;
use actix_web::{web, App, HttpServer, HttpResponse, Result, middleware::Logger};
use actix_files::Files;
use serde::{Deserialize};
use pulldown_cmark::{Parser, html};
use tera::{Tera, Context};

#[derive(Debug, Deserialize)]
struct FrontMatter {
    title: String,
    date: String,
    tags: Option<Vec<String>>,
}

#[derive(Debug)]
struct Post {
    slug: String,
    frontmatter: FrontMatter,
    content: String,
    html: String,
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

fn main() {
    println!("Starting markdown blog server...");
    
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
    
    start_server(posts);
}

#[actix_web::main]
async fn start_server(posts: Vec<Post>) -> std::io::Result<()> {
    // Create post cache
    let mut post_cache = HashMap::new();
    for post in posts {
        post_cache.insert(post.slug.clone(), post);
    }
    let post_cache = web::Data::new(post_cache);
    
    // Initialize templates
    let tera = Tera::new("templates/**/*").unwrap_or_else(|_| {
        println!("Warning: No templates found, creating empty Tera instance");
        Tera::default()
    });
    let tera_data = web::Data::new(tera);
    
    HttpServer::new(move || {
        App::new()
            .app_data(post_cache.clone())
            .app_data(tera_data.clone())
            .wrap(Logger::default())
            .route("/", web::get().to(home))
            .route("/posts/{slug}", web::get().to(post_detail))
            .service(Files::new("/static", "static"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}

async fn home(
    posts: web::Data<HashMap<String, Post>>,
    tera: web::Data<Tera>,
) -> Result<HttpResponse> {
    let mut context = Context::new();
    let mut posts_vec: Vec<&Post> = posts.values().collect();
    posts_vec.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    context.insert("posts", &posts_vec);
    
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
    posts: web::Data<HashMap<String, Post>>,
    tera: web::Data<Tera>,
) -> Result<HttpResponse> {
    let slug = path.into_inner();
    
    if let Some(post) = posts.get(&slug) {
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
        Ok(HttpResponse::NotFound().content_type("text/html").body("<h1>404 - Post Not Found</h1>"))
    }
}
