use anyhow::{Context, Result};
use clap::Parser;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_yaml;
use std::collections::HashMap;
use std::fs;
use tokio;

#[derive(Parser)]
#[command(name = "neostats")]
#[command(about = "Generate SVG visualization of GitHub language statistics")]
struct Cli {
    /// GitHub username to analyze
    username: String,
    
    /// Output SVG file path
    #[arg(short, long, default_value = "neostats.svg")]
    svg: String,
    
    /// Save language data to YAML file
    #[arg(long)]
    out: Option<String>,
    
    /// Load language data from YAML file (bypasses network requests)
    #[arg(long, value_name = "FILE")]
    r#in: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    languages_url: String,
    #[serde(default)]
    fork: bool,
}

#[derive(Debug, Deserialize)]
struct LanguageInfo {
    color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LanguageStats {
    name: String,
    percentage: f64,
    color: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    let language_stats = if let Some(input_path) = &cli.r#in {
        // Load data from YAML file, bypassing network requests
        load_language_data(input_path)?
    } else {
        // Perform network requests to fetch data
        let client = Client::new();
        
        // Fetch language colors from GitHub Linguist
        println!("Fetching language colors...");
        let language_colors = fetch_language_colors(&client).await?;
        
        // Fetch all repositories for the user
        println!("Fetching repositories for user: {}", cli.username);
        let repositories = fetch_repositories(&client, &cli.username).await?;
        
        // Filter out forks and fetch language data
        let non_fork_repos: Vec<_> = repositories.into_iter().filter(|repo| !repo.fork).collect();
        println!("Found {} non-fork repositories", non_fork_repos.len());
        
        // Aggregate language statistics
        let mut language_totals: HashMap<String, u64> = HashMap::new();
        
        for repo in &non_fork_repos {
            if let Ok(languages) = fetch_repository_languages(&client, &repo.languages_url).await {
                for (language, bytes) in languages {
                    *language_totals.entry(language).or_insert(0) += bytes;
                }
            }
        }
        
        // Calculate percentages and get top 10
        let total_bytes: u64 = language_totals.values().sum();
        if total_bytes == 0 {
            anyhow::bail!("No language data found for user {}", cli.username);
        }
        
        let mut language_stats: Vec<LanguageStats> = language_totals
            .into_iter()
            .map(|(name, bytes)| LanguageStats {
                percentage: (bytes as f64 / total_bytes as f64) * 100.0,
                color: get_language_color(&name, &language_colors),
                name,
            })
            .collect();
        
        language_stats.sort_by(|a, b| b.percentage.partial_cmp(&a.percentage).unwrap());
        language_stats.truncate(10);
        
        // Save data to YAML file if --out is specified
        if let Some(output_path) = &cli.out {
            save_language_data(&language_stats, &cli.username, output_path)?;
        }
        
        language_stats
    };
    
    // Generate SVG
    let svg = generate_svg(&cli.username, &language_stats);
    
    // Write to file
    fs::write(&cli.svg, svg).context("Failed to write SVG file")?;
    
    println!("SVG generated successfully: {}", cli.svg);
    println!("Top languages:");
    for lang in &language_stats {
        println!("  {}: {:.1}%", lang.name, lang.percentage);
    }
    
    Ok(())
}

async fn fetch_repositories(client: &Client, username: &str) -> Result<Vec<Repository>> {
    let mut repositories = Vec::new();
    let mut page = 1;
    
    loop {
        let url = format!(
            "https://api.github.com/users/{}/repos?page={}&per_page=100&type=owner",
            username, page
        );
        
        let response = client
            .get(&url)
            .header("User-Agent", "neostats/1.0")
            .send()
            .await
            .context("Failed to fetch repositories")?;
        
        if !response.status().is_success() {
            anyhow::bail!("GitHub API error: {}", response.status());
        }
        
        let repos: Vec<Repository> = response
            .json()
            .await
            .context("Failed to parse repository JSON")?;
        
        if repos.is_empty() {
            break;
        }
        
        repositories.extend(repos);
        page += 1;
    }
    
    Ok(repositories)
}

async fn fetch_repository_languages(client: &Client, languages_url: &str) -> Result<HashMap<String, u64>> {
    let response = client
        .get(languages_url)
        .header("User-Agent", "neostats/1.0")
        .send()
        .await
        .context("Failed to fetch repository languages")?;
    
    if !response.status().is_success() {
        return Ok(HashMap::new()); // Skip repositories we can't access
    }
    
    let languages: HashMap<String, u64> = response
        .json()
        .await
        .context("Failed to parse languages JSON")?;
    
    Ok(languages)
}

async fn fetch_language_colors(client: &Client) -> Result<HashMap<String, String>> {
    let url = "https://raw.githubusercontent.com/github-linguist/linguist/main/lib/linguist/languages.yml";
    
    let response = client
        .get(url)
        .header("User-Agent", "neostats/1.0")
        .send()
        .await
        .context("Failed to fetch languages.yml")?;
    
    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch languages.yml: {}", response.status());
    }
    
    let yaml_content = response
        .text()
        .await
        .context("Failed to read languages.yml content")?;
    
    let languages: HashMap<String, LanguageInfo> = serde_yaml::from_str(&yaml_content)
        .context("Failed to parse languages.yml")?;
    
    let mut color_map = HashMap::new();
    for (name, info) in languages {
        if let Some(color) = info.color {
            color_map.insert(name, color);
        }
    }
    
    Ok(color_map)
}

fn get_language_color(language: &str, color_map: &HashMap<String, String>) -> String {
    color_map
        .get(language)
        .cloned()
        .unwrap_or_else(|| "#586e75".to_string()) // Default color
}

fn save_language_data(data: &[LanguageStats], _username: &str, output_path: &str) -> Result<()> {
    let yaml_content = serde_yaml::to_string(data)
        .context("Failed to serialize language data to YAML")?;
    
    fs::write(output_path, yaml_content)
        .context("Failed to write YAML file")?;
    
    println!("Language data saved to: {}", output_path);
    Ok(())
}

fn load_language_data(input_path: &str) -> Result<Vec<LanguageStats>> {
    let yaml_content = fs::read_to_string(input_path)
        .context("Failed to read YAML file")?;
    
    let language_stats: Vec<LanguageStats> = serde_yaml::from_str(&yaml_content)
        .context("Failed to parse YAML file")?;
    
    println!("Language data loaded from: {}", input_path);
    Ok(language_stats)
}

fn generate_svg(_username: &str, languages: &[LanguageStats]) -> String {
    let width = 300;
    let height = 195;
    let padding = 20;
    let title_height = 25;
    let progress_bar_height = 8;
    let progress_bar_margin = 10;
    let item_height = 12;
    let item_spacing = 2;
    
    let mut svg = String::new();
    
    // SVG header and styles
    svg.push_str(&format!(
        r#"<svg width="{}" height="{}" viewBox="0 0 {} {}" fill="none" xmlns="http://www.w3.org/2000/svg">"#,
        width, height, width, height
    ));
    
    svg.push_str(r#"
<style>
.header { font: 600 18px 'Segoe UI', Ubuntu, Sans-Serif; fill: #2f80ed }
    .lang-name { font: 400 11px 'Segoe UI', Ubuntu, Sans-Serif; fill: #434d58 }
</style>"#);
    
    // Background card
        svg.push_str("<rect data-testid=\"card-bg\" x=\"0.5\" y=\"0.5\" rx=\"15\" ry=\"15\" height=\"99%\" stroke=\"#e4e2e2\" width=\"99%\" fill=\"#fffefe\" stroke-opacity=\"1\"/>");
    
    // Title
    svg.push_str(&format!(
        "<g data-testid=\"card-title\" transform=\"translate({}, {})\"><text x=\"0\" y=\"0\" class=\"header\" data-testid=\"header\">Most Used Languages</text></g>",
        padding, padding + 15
    ));
    
    // Progress bar background
    let bar_width = width - 2 * padding;
    let bar_width_f64 = bar_width as f64;
    let bar_y = padding + title_height + progress_bar_margin;
    svg.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"#f6f8fa\"/>",
        padding,
        bar_y,
        bar_width,
        progress_bar_height,
        progress_bar_height / 2,
        progress_bar_height / 2
    ));

    svg.push_str(&format!(
        "<defs><clipPath id=\"progress-clip\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\"/></clipPath></defs>",
        padding,
        bar_y,
        bar_width,
        progress_bar_height,
        progress_bar_height / 2,
        progress_bar_height / 2
    ));
    
    // Progress bar segments
    let mut current_x = padding as f64;
    let last_index = languages.len().saturating_sub(1);
    svg.push_str("<g clip-path=\"url(#progress-clip)\">");
    for (index, lang) in languages.iter().enumerate() {
        let mut segment_width = (lang.percentage / 100.0) * bar_width_f64;
        if index == last_index {
            let remaining = (padding as f64 + bar_width_f64) - current_x;
            segment_width = remaining.max(0.0);
        }

        if segment_width > 0.5 || index == last_index {
            svg.push_str(&format!(
                "<rect x=\"{:.3}\" y=\"{}\" width=\"{:.3}\" height=\"{}\" fill=\"{}\"/>",
                current_x, bar_y, segment_width, progress_bar_height, lang.color
            ));
        }

        current_x += segment_width;
    }
    svg.push_str("</g>");
    
    let cols = 2;
    let col_width = (width - 2 * padding) / cols;
    
    for (i, lang) in languages.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;
        let x = padding + col * col_width;
        let y = padding + title_height + progress_bar_height + progress_bar_margin * 2 + 15 + row * (item_height + item_spacing + 8);
        
        // Language circle and name
        svg.push_str(&format!(
            "<g transform=\"translate({}, {})\"><circle cx=\"5\" cy=\"6\" r=\"5\" fill=\"{}\"/><text data-testid=\"lang-name\" x=\"15\" y=\"10\" class=\"lang-name\">{} {:.1}%</text></g>",
            x, y, lang.color, lang.name, lang.percentage
        ));
    }
    
    svg.push_str("</svg>");
    svg
}
