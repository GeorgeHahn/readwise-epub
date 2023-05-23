//! # Notes
//!
//! This could potentially be a very simple html-to-epub tool instead of using
//! `percollate` if Readwise added an API to fetch their cleaned html. (Todo:
//! request that.)
//!
//! I tried directly integrating https://github.com/hipstermojo/paperoni (built
//! on https://github.com/lise-henry/epub-builder), but it emitted epub files
//! that were fairly spec-noncompliant. Code review did not give me much
//! confidence that it could be brought up to spec without a significant amount
//! of work.
//!
//! See the official EPUBCheck tool: https://www.w3.org/publishing/epubcheck/
//!
//! # Todo
//!
//! - Cache redirects
//! - Download articles as individual epubs (cache them) and then merge
//! - Make this reasonable to run nightly
//! - Attach this to something like [Koblime](https://kobli.me/) to sync each
//!   day's new articles directly to device.
//!

use itertools::Itertools;
use reqwest::{header, Method};
use serde::{Deserialize, Serialize};
use std::{process::Stdio, time::Duration};
use time::OffsetDateTime;
use tokio::{fs::File, io::AsyncWriteExt};

#[derive(Debug, Deserialize, Serialize)]
struct ListResults {
    count: u64,
    #[serde(rename = "nextPageCursor")]
    next_page_cursor: Option<String>,
    results: Vec<Item>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Item {
    title: Option<String>,
    author: Option<String>,
    site_name: Option<String>,
    source_url: String,
    image_url: Option<String>,
    summary: Option<String>,
    content: Option<String>,
    #[serde(deserialize_with = "null_to_default")]
    word_count: u32,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    // these always seem to be null
    // #[serde(deserialize_with = "null_to_default_time")]
    // published_date: Option<Date>,
    // #[serde(default)]
    // tags: Option<HashMap<String, serde_json::Value>>,
}

fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
struct Config {
    reader_token: String,
}

#[tokio::main]
async fn main() {
    let config_file = dirs::config_dir()
        .unwrap()
        .join("readwise-epub/config.toml");
    let config = config::Config::builder()
        .add_source(config::Environment::with_prefix("READWISE_EPUB").separator("_"))
        .add_source(config::File::from(config_file.as_ref()).required(false))
        .build()
        .unwrap()
        .try_deserialize::<Config>()
        .unwrap();

    let client = reqwest::Client::new();
    let mut cursor = None;
    let mut acc = vec![];

    loop {
        let uri = if let Some(cursor) = cursor {
            format!("https://readwise.io/api/v3/list/?location=new&pageCursor={cursor}")
        } else {
            String::from("https://readwise.io/api/v3/list/?location=new")
        };

        let res = client
            .request(Method::GET, uri)
            .header(
                header::AUTHORIZATION,
                format!("Token {}", config.reader_token),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .send()
            .await
            .unwrap();

        let mut res: ListResults = serde_json::from_slice(&res.bytes().await.unwrap()).unwrap();

        cursor = res.next_page_cursor;
        acc.append(&mut res.results);

        if cursor.is_none() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(60000 / 20)).await;
    }

    let output = serde_json::to_string_pretty(&acc).unwrap();
    let mut file = File::create("all_uris.json").await.unwrap();
    file.write_all(output.as_bytes()).await.unwrap();

    // todo: download PDFs separately

    acc.sort_by_key(|i| i.updated_at);

    // filter out emails
    // todo: request a way to get this content from readwise / fetch from private APIs
    let mut filtered = acc
        .into_iter()
        .filter(|val| !val.source_url.starts_with("mailto:"))
        .collect::<Vec<_>>();

    // follow redirects
    let client = reqwest::Client::new();
    for mut val in &mut filtered {
        let Ok(resp) = client.head(&val.source_url).send().await else {
            println!("Failed to HEAD {}", val.source_url);
            continue;
        };
        let resp_url = resp.url().to_string();
        if val.source_url != resp_url {
            println!("Redirected from {} to {}", val.source_url, resp_url);
            val.source_url = resp.url().to_string();
        }
    }

    // group articles by author and site
    let author_and_site = filtered
        .into_iter()
        .group_by(|i| (i.author.clone(), i.site_name.clone()));

    // retain groups of more than one article for the same author and site
    let mut groups = vec![];
    let mut remainder = vec![];
    for ((author, site), group) in &author_and_site {
        let mut group = group.collect::<Vec<_>>();
        if (author.is_some() || site.is_some()) && group.len() > 1 {
            let name = match (author, site) {
                (Some(author), Some(site)) => format!("{author} - {site}"),
                (Some(author), None) => author.clone(),
                (None, Some(site)) => site.clone(),
                (None, None) => unreachable!(),
            };

            groups.push((name, group));
        } else {
            remainder.append(&mut group);
        }
    }

    // Group remaining articles into approx 1 hour chunks
    let mut remainder = remainder
        .into_iter()
        .fold((0, vec![vec![]]), |(word_count, mut groups), item| {
            // todo: request reading time from Readwise in the API
            let wc = item.word_count;

            // group into approx. 1 hour chunks
            if word_count < 8000 {
                groups.last_mut().unwrap().push(item);
                (word_count + wc, groups)
            } else {
                groups.push(vec![item]);
                (wc, groups)
            }
        })
        .1
        .into_iter()
        .map(|group| (group.first().unwrap().updated_at.date().to_string(), group))
        .collect();

    groups.append(&mut remainder);

    let _ = tokio::fs::create_dir_all("epubs").await;

    println!("Creating {} groups of articles", groups.len());
    for (name, group) in groups {
        let mut num = 1;
        let mut filename = format!("readwise-{}.epub", name);
        let mut title = name.to_string();
        while let Ok(true) = tokio::fs::try_exists(&filename).await {
            num += 1;
            filename = format!("readwise-{name}-{num}.epub",);
            title = format!("{name} Pt. {}", num);
        }
        println!("Creating {} from {} articles", filename, group.len());
        tokio::process::Command::new("percollate.cmd")
            .arg("epub")
            .arg("--output")
            .arg(filename)
            .arg("--title")
            .arg(title)
            .arg("--author")
            .arg("readwise")
            .args(group.into_iter().map(|i| i.source_url).collect::<Vec<_>>())
            .current_dir("epubs")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();
    }
}
