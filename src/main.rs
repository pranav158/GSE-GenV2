use anyhow::{anyhow, Context, Result};
use clap::Parser;
use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

// ─── CLI ────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "gse-gen",
    about = "Goldberg Steam Emulator Settings Generator v2\nGenerates a complete steam_settings folder for any Steam game.",
    version = "2.0.0"
)]
struct Cli {
    /// Steam App ID (e.g. 1465360) or game name (e.g. "SnowRunner")
    query: String,

    /// Output directory. Defaults to "<GameName> (<AppID>)"
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Username to put in configs.user.ini
    #[arg(long, default_value = "Player")]
    username: String,

    /// SteamID64 to put in configs.user.ini
    #[arg(long, default_value = "76561198999999999")]
    steamid: String,

    /// Skip downloading achievement images
    #[arg(long)]
    no_images: bool,

    /// Skip achievements entirely
    #[arg(long)]
    no_achievements: bool,

    /// Set unlock_all=1 in configs.app.ini (unlock every DLC)
    #[arg(long)]
    unlock_all_dlc: bool,

    /// Path to the game's steam_api64.dll, used to extract steam_interfaces.txt
    #[arg(long)]
    steam_api: Option<PathBuf>,

    /// Steam Web API key (get one at https://steamcommunity.com/dev/apikey).
    /// Can also be set via STEAM_API_KEY environment variable.
    #[arg(long, env = "STEAM_API_KEY")]
    api_key: String,
}

// ─── Steam API types ─────────────────────────────────────────────────────────

#[derive(Deserialize, Debug, Clone)]
struct Achievement {
    name: String,
    #[serde(rename = "displayName")]
    display_name: String,
    description: Option<String>,
    hidden: Option<u8>,
    icon: Option<String>,
    icongray: Option<String>,
}

#[derive(Serialize, Debug)]
struct GbeAchievement {
    description: String,
    #[serde(rename = "displayName")]
    display_name: String,
    hidden: u8,
    icon: String,
    icongray: String,
    name: String,
}

// ─── Steam API helpers ────────────────────────────────────────────────────────

/// Search Steam store: returns list of (id, name)
async fn steam_search(client: &Client, query: &str) -> Result<Vec<(u64, String)>> {
    let url = format!(
        "https://store.steampowered.com/api/storesearch/?term={}&cc=us&l=english",
        urlencoding::encode(query)
    );
    let resp: Value = client.get(&url).send().await?.json().await?;
    let items = resp
        .get("items")
        .and_then(|v| v.as_array())
        .context("No items in search result")?;
    Ok(items
        .iter()
        .filter_map(|v| {
            let id = v.get("id")?.as_u64()?;
            let name = v.get("name")?.as_str()?.to_string();
            Some((id, name))
        })
        .collect())
}

/// Fetch full app details (without filters so we get packages/dlc/languages)
async fn fetch_app_details(client: &Client, app_id: u64) -> Result<Value> {
    let url = format!(
        "https://store.steampowered.com/api/appdetails/?appids={}",
        app_id
    );
    let resp: Value = client.get(&url).send().await?.json().await?;
    let entry = resp
        .get(&app_id.to_string())
        .context("App ID missing from response")?;
    if !entry
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(anyhow!("Steam returned success=false for App ID {}", app_id));
    }
    entry.get("data").cloned().context("No 'data' in response")
}

/// Fetch single DLC name (basic filter for speed)
async fn fetch_dlc_name(client: &Client, dlc_id: u64) -> String {
    let url = format!(
        "https://store.steampowered.com/api/appdetails/?appids={}&filters=basic",
        dlc_id
    );
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return format!("DLC {}", dlc_id),
    };
    let json: Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return format!("DLC {}", dlc_id),
    };
    json.get(&dlc_id.to_string())
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("DLC {}", dlc_id))
}

/// Fetch achievement schema
async fn fetch_achievements(client: &Client, app_id: u64, api_key: &str) -> Result<Vec<Achievement>> {
    let url = format!(
        "https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/?key={}&appid={}&l=english",
        api_key, app_id
    );
    let resp: Value = client.get(&url).send().await?.json().await?;
    let achievements: Vec<Achievement> = resp
        .get("game")
        .and_then(|g| g.get("availableGameStats"))
        .and_then(|s| s.get("achievements"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| serde_json::from_value::<Achievement>(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    Ok(achievements)
}

/// Fetch branch list
async fn fetch_branches(client: &Client, app_id: u64) -> Value {
    let url = format!(
        "https://api.steampowered.com/ISteamApps/GetAppBranches/v1/?appid={}",
        app_id
    );
    let default = || json!({ "public": { "BuildID": "0", "TimeUpdated": "0" } });
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return default(),
    };
    let v: Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return default(),
    };
    let branches = match v.get("branches").and_then(|b| b.as_array()) {
        Some(arr) => arr,
        None => return default(),
    };
    let mut obj = serde_json::Map::new();
    for b in branches {
        let name = match b.get("name").and_then(|n| n.as_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let mut entry = serde_json::Map::new();
        if let Some(bid) = b.get("buildid").and_then(|v| v.as_u64()) {
            entry.insert("BuildID".into(), json!(bid.to_string()));
        }
        if let Some(tu) = b.get("timeupdated").and_then(|v| v.as_u64()) {
            entry.insert("TimeUpdated".into(), json!(tu.to_string()));
        }
        if let Some(pwd) = b.get("pwdrequired").and_then(|v| v.as_u64()) {
            entry.insert("PasswordRequired".into(), json!(pwd));
        }
        obj.insert(name, Value::Object(entry));
    }
    Value::Object(obj)
}

/// Fetch depot IDs from package details
async fn fetch_depots(client: &Client, app_data: &Value) -> Vec<u64> {
    let pkg_ids: Vec<u64> = app_data
        .get("packages")
        .and_then(|p| p.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
        .unwrap_or_default();

    let mut depots: BTreeSet<u64> = BTreeSet::new();

    for pkg_id in pkg_ids.iter().take(5) {
        let url = format!(
            "https://store.steampowered.com/api/packagedetails/?packageids={}",
            pkg_id
        );
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(json) = resp.json::<Value>().await {
                if let Some(arr) = json
                    .get(&pkg_id.to_string())
                    .and_then(|v| v.get("data"))
                    .and_then(|d| d.get("depot_ids"))
                    .and_then(|d| d.as_array())
                {
                    for v in arr {
                        if let Some(id) = v.as_u64() {
                            depots.insert(id);
                        }
                    }
                }
            }
        }
    }
    depots.into_iter().collect()
}

/// Download bytes from URL
async fn download(client: &Client, url: &str) -> Result<Vec<u8>> {
    let bytes = client.get(url).send().await?.bytes().await?;
    Ok(bytes.to_vec())
}

// ─── Language mapping ─────────────────────────────────────────────────────────

fn map_language(raw: &str) -> Option<&'static str> {
    match raw.trim().to_lowercase().as_str() {
        "english" => Some("english"),
        "french" => Some("french"),
        "italian" => Some("italian"),
        "german" => Some("german"),
        "spanish - spain" | "spanish" => Some("spanish"),
        "spanish - latin america" => Some("latam"),
        "czech" => Some("czech"),
        "danish" => Some("danish"),
        "dutch" => Some("dutch"),
        "finnish" => Some("finnish"),
        "greek" => Some("greek"),
        "hungarian" => Some("hungarian"),
        "indonesian" => Some("indonesian"),
        "japanese" => Some("japanese"),
        "korean" => Some("koreana"),
        "norwegian" => Some("norwegian"),
        "polish" => Some("polish"),
        "portuguese - portugal" | "portuguese" => Some("portuguese"),
        "portuguese - brazil" => Some("brazilian"),
        "romanian" => Some("romanian"),
        "russian" => Some("russian"),
        "simplified chinese" => Some("schinese"),
        "traditional chinese" => Some("tchinese"),
        "swedish" => Some("swedish"),
        "thai" => Some("thai"),
        "turkish" => Some("turkish"),
        "ukrainian" => Some("ukrainian"),
        "vietnamese" => Some("vietnamese"),
        "arabic" => Some("arabic"),
        "bulgarian" => Some("bulgarian"),
        _ => None,
    }
}

fn parse_languages(raw: &str) -> Vec<&'static str> {
    // Strip HTML tags like <strong>*</strong> and <br> variants
    let no_html = Regex::new(r"<[^>]+>").unwrap().replace_all(raw, "");
    let clean = no_html
        .replace("languages with full audio support", "")
        .replace("  ", " ");
    let mut langs = Vec::new();
    for part in clean.split(',') {
        if let Some(m) = map_language(part.trim()) {
            if !langs.contains(&m) {
                langs.push(m);
            }
        }
    }
    langs
}

// ─── steam_interfaces.txt extraction ─────────────────────────────────────────

fn extract_interfaces(dll_path: &Path) -> Result<Vec<String>> {
    let bytes = fs::read(dll_path)?;
    let pat = concat!(
        r"(?:SteamClient|SteamGameServerStats|SteamGameServer|SteamMatchMakingServers",
        r"|SteamMatchMaking|SteamUser|SteamFriends|SteamUtils|SteamNetworking",
        r"|STEAMUSERSTATS_INTERFACE_VERSION|STEAMAPPS_INTERFACE_VERSION",
        r"|STEAMREMOTESTORAGE_INTERFACE_VERSION|STEAMSCREENSHOTS_INTERFACE_VERSION",
        r"|STEAMHTTP_INTERFACE_VERSION|STEAMUNIFIEDMESSAGES_INTERFACE_VERSION",
        r"|STEAMCONTROLLER_INTERFACE_VERSION|SteamController",
        r"|STEAMUGC_INTERFACE_VERSION|STEAMAPPLIST_INTERFACE_VERSION",
        r"|STEAMMUSIC_INTERFACE_VERSION|STEAMMUSICREMOTE_INTERFACE_VERSION",
        r"|STEAMHTMLSURFACE_INTERFACE_VERSION_|STEAMINVENTORY_INTERFACE_V",
        r"|STEAMVIDEO_INTERFACE_V|SteamMasterServerUpdater)\d{3}",
    );
    let pattern = Regex::new(pat)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut found: BTreeSet<String> = BTreeSet::new();
    for m in pattern.find_iter(&text) {
        found.insert(m.as_str().to_string());
    }
    Ok(found.into_iter().collect())
}

// ─── UI helpers ───────────────────────────────────────────────────────────────

fn spinner() -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

fn bar(len: u64, mp: &MultiProgress) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(len));
    pb.set_style(
        ProgressStyle::default_bar()
            .template("    {bar:42.cyan/blue} {pos:>3}/{len:3}  {msg:.dim}")
            .unwrap()
            .progress_chars("█▓░"),
    );
    pb
}

fn ok(msg: &str) {
    println!("  {} {}", "✓".green().bold(), msg);
}

fn info(msg: &str) {
    println!("  {} {}", "→".cyan(), msg);
}

fn warn(msg: &str) {
    println!("  {} {}", "!".yellow(), msg);
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    println!();
    println!("{}", "  ╔══════════════════════════════════════════════╗".bright_blue());
    println!("{}", "  ║   GSE Generator v2  ·  GBE Settings Builder  ║".bright_blue().bold());
    println!("{}", "  ╚══════════════════════════════════════════════╝".bright_blue());
    println!();

    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(30))
        .build()?;

    // ── Resolve App ID & Name ─────────────────────────────────────────────────
    let (app_id, game_name) = if let Ok(id) = args.query.trim().parse::<u64>() {
        let sp = spinner();
        sp.set_message(format!("Fetching details for App ID {}...", id));
        let data = fetch_app_details(&client, id).await?;
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        sp.finish_and_clear();
        ok(&format!(
            "Found  {} {}",
            name.yellow().bold(),
            format!("({})", id).dimmed()
        ));
        (id, name)
    } else {
        let sp = spinner();
        sp.set_message(format!("Searching for \"{}\"...", args.query));
        let results = steam_search(&client, &args.query).await?;
        sp.finish_and_clear();
        if results.is_empty() {
            return Err(anyhow!("No games found for \"{}\"", args.query));

        }
        // Show top results if ambiguous
        if results.len() > 1 {
            println!("  {} Multiple matches — using top result:", "→".cyan());
            for (i, (id, name)) in results.iter().take(5).enumerate() {
                println!(
                    "      {}  {} {}",
                    format!("[{}]", i + 1).dimmed(),
                    name,
                    format!("({})", id).dimmed()
                );
            }
            println!();
        }
        let (id, name) = results.into_iter().next().unwrap();
        ok(&format!(
            "Found  {} {}",
            name.yellow().bold(),
            format!("({})", id).dimmed()
        ));
        (id, name)
    };

    // ── Full app details ──────────────────────────────────────────────────────
    let sp = spinner();
    sp.set_message("Fetching full app data...");
    let app_data = fetch_app_details(&client, app_id).await?;
    sp.finish_and_clear();

    // ── Create output directories ─────────────────────────────────────────────
    let safe_name: String = game_name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();

    let out_dir = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{} ({})", safe_name, app_id)));
    let settings = out_dir.join("steam_settings");
    let ach_images = settings.join("achievement_images");
    let fonts_dir = settings.join("fonts");
    let sounds_dir = settings.join("sounds");

    fs::create_dir_all(&settings)?;
    fs::create_dir_all(&ach_images)?;
    fs::create_dir_all(&fonts_dir)?;
    fs::create_dir_all(&sounds_dir)?;
    ok(&format!("Output  {}", out_dir.display().to_string().cyan()));
    println!();

    // ── steam_appid.txt ───────────────────────────────────────────────────────
    fs::write(settings.join("steam_appid.txt"), format!("{}\n", app_id))?;
    ok("steam_appid.txt");

    // ── supported_languages.txt ───────────────────────────────────────────────
    let raw_langs = app_data
        .get("supported_languages")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let langs = parse_languages(raw_langs);
    fs::write(
        settings.join("supported_languages.txt"),
        langs.join("\n") + "\n",
    )?;
    ok(&format!(
        "supported_languages.txt  ({} languages)",
        langs.len()
    ));

    // ── DLCs → configs.app.ini ────────────────────────────────────────────────
    let dlc_ids: Vec<u64> = app_data
        .get("dlc")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
        .unwrap_or_default();

    info(&format!("Fetching {} DLC name(s)...", dlc_ids.len()));

    let mp = MultiProgress::new();
    let mut dlc_entries: Vec<(u64, String)> = Vec::new();

    if !dlc_ids.is_empty() {
        let pb = bar(dlc_ids.len() as u64, &mp);
        for chunk in dlc_ids.chunks(5) {
            let tasks: Vec<_> = chunk
                .iter()
                .map(|&id| {
                    let c = client.clone();
                    async move { (id, fetch_dlc_name(&c, id).await) }
                })
                .collect();
            let results = futures::future::join_all(tasks).await;
            for (id, name) in results {
                pb.set_message(name.clone());
                dlc_entries.push((id, name));
                pb.inc(1);
            }
        }
        pb.finish_and_clear();
    }

    let unlock = if args.unlock_all_dlc { "1" } else { "0" };
    let mut app_ini = format!("[app::dlcs]\nunlock_all={}\n", unlock);
    for (id, name) in &dlc_entries {
        app_ini.push_str(&format!("{}={}\n", id, name));
    }
    fs::write(settings.join("configs.app.ini"), &app_ini)?;
    ok(&format!(
        "configs.app.ini          ({} DLCs)",
        dlc_entries.len()
    ));

    // ── configs.user.ini ──────────────────────────────────────────────────────
    let default_lang = langs.first().copied().unwrap_or("english");
    fs::write(
        settings.join("configs.user.ini"),
        format!(
            "[user::general]\naccount_name={}\naccount_steamid={}\nlanguage={}\n",
            args.username, args.steamid, default_lang
        ),
    )?;
    ok(&format!(
        "configs.user.ini         (name={}, lang={})",
        args.username, default_lang
    ));

    // ── configs.main.ini ──────────────────────────────────────────────────────
    fs::write(
        settings.join("configs.main.ini"),
        "[connectivity]\nlan_only=0\noffline=0\ndisable_networking=0\n\n[main]\n",
    )?;
    ok("configs.main.ini");

    // ── configs.overlay.ini ───────────────────────────────────────────────────
    fs::write(
        settings.join("configs.overlay.ini"),
        "[overlay::general]\nenable_experimental_overlay=1\n\n\
         [overlay::appearance]\nNotification_Rounding=10.0\n\
         Notification_Margin_x=5.0\nNotification_Margin_y=5.0\n\
         Notification_Animation=0.35\nPosAchievement=bot_right\n",
    )?;
    ok("configs.overlay.ini");

    // ── branches.json ─────────────────────────────────────────────────────────
    let sp = spinner();
    sp.set_message("Fetching branch info...");
    let branches = fetch_branches(&client, app_id).await;
    sp.finish_and_clear();
    fs::write(
        settings.join("branches.json"),
        serde_json::to_string_pretty(&branches)?,
    )?;
    ok(&format!(
        "branches.json            ({} branch(es))",
        branches.as_object().map(|o| o.len()).unwrap_or(0)
    ));

    // ── depots.txt ────────────────────────────────────────────────────────────
    let sp = spinner();
    sp.set_message("Fetching depot IDs...");
    let depots = fetch_depots(&client, &app_data).await;
    sp.finish_and_clear();

    if depots.is_empty() {
        warn("depots.txt — no depots found (skipped)");
    } else {
        let depot_str: String = depots
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(settings.join("depots.txt"), &depot_str)?;
        ok(&format!(
            "depots.txt               ({} depots)",
            depots.len()
        ));
    }

    // ── achievements.json + achievement_images/ ───────────────────────────────
    if !args.no_achievements {
        let sp = spinner();
        sp.set_message("Fetching achievements schema...");
        let ach_result = fetch_achievements(&client, app_id, &args.api_key).await;
        sp.finish_and_clear();

        match ach_result {
            Ok(achievements) if !achievements.is_empty() => {
                let total = achievements.len();
                let pb = bar(total as u64, &mp);
                pb.set_message("downloading...");

                let mut gbe_achs: Vec<GbeAchievement> = Vec::new();

                for ach in &achievements {
                    let icon_file = ach
                        .icon
                        .as_deref()
                        .and_then(|u| u.rsplit('/').next())
                        .unwrap_or("unknown.jpg")
                        .to_string();
                    let icongray_file = ach
                        .icongray
                        .as_deref()
                        .and_then(|u| u.rsplit('/').next())
                        .unwrap_or("unknown_gray.jpg")
                        .to_string();

                    if !args.no_images {
                        if let Some(url) = &ach.icon {
                            let dest = ach_images.join(&icon_file);
                            if !dest.exists() {
                                if let Ok(bytes) = download(&client, url).await {
                                    let _ = fs::write(&dest, &bytes);
                                }
                            }
                        }
                        if let Some(url) = &ach.icongray {
                            let dest = ach_images.join(&icongray_file);
                            if !dest.exists() {
                                if let Ok(bytes) = download(&client, url).await {
                                    let _ = fs::write(&dest, &bytes);
                                }
                            }
                        }
                    }

                    gbe_achs.push(GbeAchievement {
                        description: ach.description.clone().unwrap_or_default(),
                        display_name: ach.display_name.clone(),
                        hidden: ach.hidden.unwrap_or(0),
                        icon: format!("achievement_images/{}", icon_file),
                        icongray: format!("achievement_images/{}", icongray_file),
                        name: ach.name.clone(),
                    });

                    pb.set_message(ach.display_name.chars().take(40).collect::<String>());
                    pb.inc(1);
                }

                pb.finish_and_clear();
                fs::write(
                    settings.join("achievements.json"),
                    serde_json::to_string_pretty(&gbe_achs)?,
                )?;
                ok(&format!(
                    "achievements.json        ({} achievements{})",
                    total,
                    if args.no_images { "" } else { " + images" }
                ));
            }
            Ok(_) => warn("achievements.json — no achievements for this game"),
            Err(e) => warn(&format!("achievements.json — {}", e)),
        }
    }

    // ── steam_interfaces.txt (optional) ───────────────────────────────────────
    if let Some(dll) = &args.steam_api {
        info(&format!(
            "Extracting interfaces from {}...",
            dll.display()
        ));
        match extract_interfaces(dll) {
            Ok(ifaces) if !ifaces.is_empty() => {
                fs::write(
                    settings.join("steam_interfaces.txt"),
                    ifaces.join("\n") + "\n",
                )?;
                ok(&format!(
                    "steam_interfaces.txt     ({} interfaces)",
                    ifaces.len()
                ));
            }
            Ok(_) => warn("steam_interfaces.txt — no interfaces found in DLL"),
            Err(e) => warn(&format!("steam_interfaces.txt — {}", e)),
        }
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    println!();
    println!(
        "{}",
        "  ╔══════════════════════════════════════════════╗".green()
    );
    println!(
        "  {}  {}  {}",
        "║".green(),
        format!(
            "Done!  {}  ({})",
            game_name.yellow().bold(),
            app_id.to_string().dimmed()
        ),
        "║".green()
    );
    println!(
        "{}",
        "  ╚══════════════════════════════════════════════╝".green()
    );
    println!("  {}", out_dir.display().to_string().cyan());
    println!();

    Ok(())
}
