use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use rdc_engine::Engine;
use rdc_model::{Torrent, TorrentStatus, paths, settings::Settings};
use serde_json::{Value, json};

pub(super) fn save_path(settings: &Settings) -> String {
    let path = if settings.download_client.mapped_path.trim().is_empty() {
        &settings.download_client.download_path
    } else {
        &settings.download_client.mapped_path
    };
    format!(
        "{}{sep}",
        path.trim_end_matches(['\\', '/']),
        sep = std::path::MAIN_SEPARATOR
    )
}

/// Directory of the torrent's category as the media manager sees it, the layout the engine
/// writes to below the download directory.
fn torrent_path(torrent: &Torrent, settings: &Settings) -> String {
    let root = save_path(settings);
    let category = paths::category_directory(torrent);
    if category.as_os_str().is_empty() {
        root
    } else {
        Path::new(&root)
            .join(category)
            .to_string_lossy()
            .into_owned()
    }
}

/// Plain components of a provider file path; `None` when it has none or any other kind.
fn plain_relative(path: &str) -> Option<PathBuf> {
    let path = PathBuf::from(path.trim_matches(['/', '\\']).replace('\\', "/"));
    (path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
        && !path.as_os_str().is_empty())
    .then_some(path)
}

fn content_path(torrent: &Torrent, settings: &Settings) -> String {
    let path = torrent_path(torrent, settings);
    let Some(name) = torrent.rd_name.as_deref().and_then(paths::safe_component) else {
        return path;
    };
    let files = torrent.files();
    if torrent.client_kind == Some(rdc_model::Provider::AllDebrid)
        && let [file] = files.as_slice()
        && let Some(relative) = plain_relative(&file.path)
    {
        return Path::new(&path)
            .join(relative)
            .to_string_lossy()
            .into_owned();
    }
    if let Some(existing) = existing_content_path(torrent, Path::new(&path)) {
        return existing;
    }
    let selected = files
        .iter()
        .filter(|file| file.selected)
        .collect::<Vec<_>>();
    let file_name = if torrent.client_kind != Some(rdc_model::Provider::TorBox)
        && let [file] = selected.as_slice()
        && !file.path.trim_matches(['/', '\\']).contains(['/', '\\'])
        && Path::new(&file.path)
            .file_stem()
            .is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case(&name))
        && let Some(file_name) = paths::safe_component(file.path.trim_matches(['/', '\\']))
    {
        file_name
    } else {
        name
    };
    format!(
        "{}{sep}",
        Path::new(&path).join(file_name).display(),
        sep = std::path::MAIN_SEPARATOR
    )
}

fn existing_content_path(torrent: &Torrent, root: &Path) -> Option<String> {
    if torrent.completed.is_none() || !root.is_dir() {
        return None;
    }
    let selected: Vec<PathBuf> = torrent
        .files()
        .iter()
        .filter(|file| file.selected)
        .filter_map(|file| plain_relative(&file.path))
        .collect();
    let downloaded: Vec<&str> = torrent
        .downloads
        .iter()
        .filter_map(|download| download.file_name.as_deref())
        .filter_map(|name| Path::new(name).file_name().and_then(|name| name.to_str()))
        .filter(|name| !name.is_empty())
        .collect();
    if selected.is_empty() && downloaded.is_empty() {
        return None;
    }
    let mut candidates = Vec::new();
    if let Some(name) = torrent.rd_name.as_deref().and_then(paths::safe_component) {
        candidates.push(root.join(name));
    }
    candidates.extend(downloaded.iter().map(|name| root.join(name)));
    for path in &selected {
        if let Some(name) = path.file_name() {
            candidates.push(root.join(name));
        }
        if let Some(Component::Normal(first)) = path.components().next() {
            candidates.push(root.join(first));
        }
    }
    if let Ok(children) = std::fs::read_dir(root) {
        candidates.extend(children.filter_map(Result::ok).map(|entry| entry.path()));
    }
    let mut seen = std::collections::HashSet::new();
    for candidate in candidates {
        if !candidate.is_dir() || !seen.insert(candidate.to_string_lossy().to_ascii_lowercase()) {
            continue;
        }
        if selected.iter().any(|path| candidate.join(path).is_file())
            || downloaded.iter().any(|name| candidate.join(name).is_file())
        {
            return Some(format!(
                "{}{sep}",
                candidate.display(),
                sep = std::path::MAIN_SEPARATOR
            ));
        }
    }
    None
}

fn unix(timestamp: jiff::Timestamp) -> i64 {
    timestamp.as_second()
}
fn now() -> i64 {
    unix(jiff::Timestamp::now())
}
fn speed_stats(torrent: &Torrent, engine: &Engine) -> (i64, i64, i64) {
    let mut done = 0_i64;
    let mut total = 0_i64;
    let mut speed = 0_i64;
    let files = std::cell::OnceCell::new();
    for download in &torrent.downloads {
        let stats = engine.download_stats(download.download_id);
        let bytes_total = i64::try_from(stats.bytes_total).unwrap_or(i64::MAX);
        let bytes_total = if bytes_total == 0 {
            paths::file_name(download)
                .filter(|name| !name.trim().is_empty())
                .and_then(|name| {
                    let mut matches = files
                        .get_or_init(|| torrent.files())
                        .iter()
                        .filter(|file| !file.path.trim().is_empty() && file.path.ends_with(&name));
                    let file = matches.next()?;
                    (matches.next().is_none() && file.bytes > 0).then_some(file.bytes)
                })
                .unwrap_or_default()
        } else {
            bytes_total
        };
        let bytes_done = if download.download_finished.is_some() || download.completed.is_some() {
            bytes_total
        } else {
            i64::try_from(stats.bytes_done).unwrap_or(i64::MAX)
        };
        done = done.saturating_add(bytes_done);
        total = total.saturating_add(bytes_total);
        speed = speed.saturating_add(i64::try_from(stats.speed).unwrap_or(i64::MAX));
    }
    let count = i64::try_from(torrent.downloads.len()).unwrap_or(i64::MAX);
    (done, total, if count == 0 { 0 } else { speed / count })
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "the qBittorrent progress and ETA wire fields are floating-point ratios"
)]
pub(super) fn info(
    torrent: &Torrent,
    engine: &Engine,
    settings: &Settings,
    priority: usize,
) -> Value {
    let bytes_total = torrent.rd_size.unwrap_or_default();
    let percent = torrent.rd_progress.unwrap_or_default().clamp(0, 100);
    let rd_progress = f64::from(i32::try_from(percent).unwrap_or(100)) / 100.0;
    let bytes_done =
        i64::try_from(i128::from(bytes_total) * i128::from(percent) / 100).unwrap_or(i64::MAX);
    let mut speed = torrent.rd_speed.unwrap_or_default();
    let mut progress = rd_progress;
    if torrent.completed.is_some() {
        progress = 1.0;
    } else if !torrent.downloads.is_empty() {
        let (done, total, average) = speed_stats(torrent, engine);
        speed = average;
        let dl_progress = if total > 0 {
            (done as f64 / total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        progress = if rd_progress >= 1.0 && dl_progress >= 1.0 {
            1.0
        } else {
            rd_progress.midpoint(dl_progress)
        };
    }
    if torrent.completed.is_none() && progress >= 1.0 {
        progress = 0.99;
    }
    let current = now();
    let start = torrent
        .retry
        .filter(|retry| *retry > torrent.added)
        .unwrap_or(torrent.added);
    let elapsed = current.saturating_sub(unix(start));
    let eta = if progress > 0.0 && progress < 1.0 {
        ((elapsed as f64 / progress) - elapsed as f64) as i64
    } else {
        0
    };
    let state = if torrent
        .error
        .as_deref()
        .is_some_and(|error| !error.trim().is_empty())
    {
        "error"
    } else if torrent.completed.is_some() {
        "pausedUP"
    } else if torrent.rd_status == Some(TorrentStatus::Downloading)
        && torrent.rd_seeders.is_some_and(|seeders| seeders < 1)
    {
        "stalledDL"
    } else {
        "downloading"
    };
    json!({
        "added_on":unix(torrent.added),"amount_left":bytes_total.saturating_sub(bytes_done),"auto_tmm":false,
        "availability":2,"category":torrent.category.as_deref().unwrap_or(""),"completed":bytes_done,
        "completion_on":torrent.completed.map(unix),"content_path":content_path(torrent,settings),"dl_limit":-1,
        "dlspeed":speed,"downloaded":bytes_done,"downloaded_session":bytes_done,"eta":eta,
        "f_l_piece_prio":false,"force_start":false,"hash":torrent.hash,"last_activity":current,
        "magnet_uri":"","max_ratio":-1,"max_seeding_time":-1,"name":torrent.rd_name,"num_complete":10,
        "num_incomplete":0,"num_leechs":1,"num_seeds":torrent.rd_seeders.unwrap_or(1),
        "priority":priority,"progress":progress as f32,"ratio":1,"ratio_limit":1,
        "save_path":torrent_path(torrent,settings),"seeding_time_limit":1,"seen_complete":current,
        "seq_dl":false,"size":bytes_total,"state":state,"super_seeding":false,"tags":"",
        "time_active":elapsed,"total_size":bytes_total,"tracker":"udp://tracker.opentrackr.org:1337",
        "up_limit":-1,"uploaded":bytes_done,"uploaded_session":bytes_done,"upspeed":speed
    })
}

pub(super) fn files(torrent: &Torrent) -> Value {
    Value::Array(torrent.files().iter().enumerate().map(|(index, file)| {
        json!({"index":index,"name":file.path,"size":file.bytes,
            "progress":if file.selected && (torrent.completed.is_some() || torrent.rd_status == Some(TorrentStatus::Finished)) { 1.0_f32 } else { 0.0_f32 },
            "priority":i32::from(file.selected),"is_seed":false})
    }).collect())
}

pub(super) fn properties(torrent: &Torrent, engine: &Engine, settings: &Settings) -> Value {
    let (done, total, speed) = if torrent.downloads.is_empty() {
        let total = torrent.rd_size.unwrap_or_default();
        let percent = torrent.rd_progress.unwrap_or_default().clamp(0, 100);
        let done = i64::try_from(i128::from(total) * i128::from(percent) / 100).unwrap_or(i64::MAX);
        (done, total, torrent.rd_speed.unwrap_or_default())
    } else {
        speed_stats(torrent, engine)
    };
    let current = now();
    json!({"addition_date":unix(torrent.added),"comment":"rd-client","completion_date":torrent.completed.map_or(-1,unix),
        "is_private":false,"created_by":"rd-client","creation_date":unix(torrent.added),
        "dl_limit":-1,"dl_speed":speed,"dl_speed_avg":speed,"eta":0,"last_seen":current,
        "nb_connections":0,"nb_connections_limit":100,"peers":0,"peers_total":2,
        "piece_size":total,"pieces_have":torrent.downloads.iter().filter(|d|d.completed.is_some()).count(),
        "pieces_num":torrent.downloads.len(),"reannounce":0,"save_path":torrent_path(torrent,settings),
        "seeding_time":1,"seeds":100,"seeds_total":100,"share_ratio":9999,
        "time_elapsed":current.saturating_sub(unix(torrent.added)),"total_downloaded":done,
        "total_downloaded_session":done,"total_size":total,"total_uploaded":done,
        "total_uploaded_session":done,"total_wasted":0,"up_limit":-1,"up_speed":speed,"up_speed_avg":speed})
}

pub(super) fn matches_filter(torrent: &Value, filter: Option<&str>) -> bool {
    let filter = filter.unwrap_or_default().to_ascii_lowercase();
    let state = torrent["state"].as_str().unwrap_or_default();
    let speed = torrent["dlspeed"].as_i64().unwrap_or_default();
    let upload = torrent["upspeed"].as_i64().unwrap_or_default();
    let progress = torrent["progress"].as_f64().unwrap_or_default();
    match filter.as_str() {
        "" | "all" => true,
        "downloading" => progress < 1.0 && state != "error",
        "completed" => progress >= 1.0 || !torrent["completion_on"].is_null(),
        "paused" => state.starts_with("paused"),
        "active" => speed > 0 || upload > 0,
        "inactive" => speed <= 0 && upload <= 0,
        "resumed" => !state.starts_with("paused") && state != "error",
        "stalled" => matches!(state, "stalledDL" | "stalledUP"),
        "stalled_uploading" => state == "stalledUP",
        "stalled_downloading" => state == "stalledDL",
        "errored" => state == "error",
        _ => state.eq_ignore_ascii_case(&filter),
    }
}

pub(super) fn categories(torrents: &[Torrent], settings: &Settings) -> Value {
    let mut names: BTreeMap<String, Value> = BTreeMap::new();
    let root = save_path(settings);
    for name in torrents
        .iter()
        .filter_map(|torrent| {
            torrent
                .category
                .as_deref()
                .filter(|v| !v.trim().is_empty())
                .map(str::to_lowercase)
        })
        .chain(
            settings
                .general
                .categories
                .as_deref()
                .unwrap_or_default()
                .split(',')
                .filter(|v| !v.is_empty())
                .map(|v| v.trim().to_owned()),
        )
    {
        if !names.keys().any(|key| key.eq_ignore_ascii_case(&name)) {
            let save = Path::new(&root).join(&name).to_string_lossy().into_owned();
            names.insert(name.clone(), json!({"name":name,"savePath":save}));
        }
    }
    json!(names)
}

pub(super) fn transfer(engine: &Engine) -> Value {
    json!({"connection_status":"connected","dht_nodes":0,"dl_info_data":engine.session_downloaded_bytes(),
        "dl_info_speed":engine.active_download_speed_sum(),"dl_rate_limit":engine.settings().download_client.max_speed,
        "up_info_data":0,"up_info_speed":0,"up_rate_limit":0})
}

pub(super) fn sync(torrents: &[Torrent], engine: &Engine, settings: &Settings) -> Value {
    let mut values = serde_json::Map::new();
    for (index, torrent) in torrents.iter().enumerate() {
        values.insert(
            torrent.hash.clone(),
            info(torrent, engine, settings, index + 1),
        );
    }
    let mut state = serde_json::Map::new();
    for key in [
        "alltime_dl",
        "alltime_ul",
        "average_time_queue",
        "connection_status",
        "dht_nodes",
        "dl_info_data",
        "dl_rate_limit",
        "free_space_on_disk",
        "global_ratio",
        "queued_io_jobs",
        "queueing",
        "read_cache_hits",
        "read_cache_overload",
        "refresh_interval",
        "total_buffers_size",
        "total_peer_connections",
        "total_queued_size",
        "total_wasted_session",
        "up_info_data",
        "up_rate_limit",
        "use_alt_speed_limits",
        "write_cache_overload",
    ] {
        state.insert(key.to_owned(), Value::Null);
    }
    state.insert(
        "dl_info_speed".to_owned(),
        json!(engine.active_download_speed_sum()),
    );
    state.insert("up_info_speed".to_owned(), json!(0));
    json!({"categories":categories(torrents,settings),"full_update":true,"rid":0,"server_state":state,
        "tags":null,"torrents":values,"trackers":{}})
}

#[cfg(not(test))]
pub(super) const DEQUEUE_TIMEOUT: Duration = Duration::from_secs(90);
#[cfg(test)]
pub(super) const DEQUEUE_TIMEOUT: Duration = Duration::from_millis(250);

#[cfg(test)]
mod tests {
    use super::*;
    use rdc_engine::testing::TestEngine;
    use rdc_model::{Download, Provider, RemoteFile};

    #[tokio::test]
    async fn torrent_models_report_completion_and_selected_file_paths() {
        let fixture = TestEngine::new().await.unwrap();
        let settings = fixture.engine.settings();
        let name = "Film";
        let mut torrent = Torrent {
            hash: "abc".to_owned(),
            rd_name: Some(name.to_owned()),
            rd_size: Some(1000),
            rd_progress: Some(100),
            added: jiff::Timestamp::now(),
            ..Torrent::default()
        };
        torrent.set_files(&[
            RemoteFile {
                id: 45,
                path: "/Film.mkv".to_owned(),
                bytes: 1000,
                selected: true,
                download_link: None,
            },
            RemoteFile {
                id: 46,
                path: "/Film.nfo".to_owned(),
                bytes: 2,
                selected: false,
                download_link: None,
            },
        ]);
        let pending = info(&torrent, &fixture.engine, &settings, 1);
        assert!(pending["progress"].as_f64().unwrap() < 1.0);
        assert_eq!(
            pending["content_path"],
            format!(
                "{}Film.mkv{}",
                save_path(&settings),
                std::path::MAIN_SEPARATOR
            )
        );
        assert_eq!(files(&torrent)[0]["index"], 0);
        assert_eq!(files(&torrent)[0]["priority"], 1);
        assert_eq!(files(&torrent)[1]["priority"], 0);
        torrent.rd_progress = Some(42);
        assert_eq!(
            properties(&torrent, &fixture.engine, &settings)["total_downloaded"],
            420
        );
        torrent.rd_progress = Some(100);
        torrent.completed = Some(jiff::Timestamp::now());
        let finished = info(&torrent, &fixture.engine, &settings, 1);
        assert_eq!(finished["progress"], 1.0);
        assert_eq!(files(&torrent)[0]["progress"], 1.0);
        torrent.downloads.push(Download {
            file_name: Some("Film.mkv".to_owned()),
            link: Some("https://example.com/Film.mkv".to_owned()),
            download_finished: Some(jiff::Timestamp::now()),
            completed: Some(jiff::Timestamp::now()),
            ..Download::default()
        });
        assert_eq!(
            properties(&torrent, &fixture.engine, &settings)["total_downloaded"],
            1000
        );
        torrent.client_kind = Some(Provider::TorBox);
        assert_eq!(
            info(&torrent, &fixture.engine, &settings, 1)["content_path"],
            format!("{}Film{}", save_path(&settings), std::path::MAIN_SEPARATOR)
        );
    }

    #[tokio::test]
    async fn completed_content_paths_follow_the_existing_download_root() {
        let fixture = TestEngine::new().await.unwrap();
        let settings = fixture.engine.settings();
        let root = Path::new(&settings.download_client.mapped_path);
        let nested = root.join("tracker - X.mkv");
        std::fs::create_dir_all(nested.join("X")).unwrap();
        std::fs::write(nested.join("X/X.mkv"), b"data").unwrap();
        let mut torrent = Torrent {
            rd_name: Some("tracker - X".to_owned()),
            completed: Some(jiff::Timestamp::now()),
            ..Torrent::default()
        };
        torrent.set_files(&[RemoteFile {
            id: 5,
            path: "/X/X.mkv".to_owned(),
            bytes: 4,
            selected: true,
            download_link: None,
        }]);
        assert_eq!(
            content_path(&torrent, &settings),
            format!("{}{}", nested.display(), std::path::MAIN_SEPARATOR)
        );
    }

    #[tokio::test]
    async fn content_paths_stay_below_the_download_root_like_the_files() {
        let fixture = TestEngine::new().await.unwrap();
        let settings = fixture.engine.settings();
        let root = save_path(&settings);
        let sep = std::path::MAIN_SEPARATOR;
        for (category, name, directory) in [
            (
                Some("../x"),
                "../../media/tv/Show",
                format!("x{sep}mediatvShow"),
            ),
            (Some("/abs"), "/etc", format!("abs{sep}etc")),
            (Some("a/b"), "Film", format!("ab{sep}Film")),
            (Some(".."), "Film", "Film".to_owned()),
        ] {
            let torrent = Torrent {
                category: category.map(str::to_owned),
                rd_name: Some(name.to_owned()),
                ..Torrent::default()
            };
            let expected = paths::torrent_directory(&torrent).unwrap();
            assert_eq!(expected, Path::new(&directory), "{category:?} {name}");
            assert_eq!(
                content_path(&torrent, &settings),
                format!("{}{sep}", Path::new(&root).join(&expected).display()),
                "{category:?} {name}"
            );
            assert!(
                torrent_path(&torrent, &settings).starts_with(root.trim_end_matches(sep)),
                "{category:?}"
            );
        }
        let mut torrent = Torrent {
            rd_name: Some("Film".to_owned()),
            client_kind: Some(Provider::AllDebrid),
            ..Torrent::default()
        };
        torrent.set_files(&[RemoteFile {
            id: 1,
            path: "../../etc/passwd".to_owned(),
            bytes: 1,
            selected: true,
            download_link: None,
        }]);
        assert_eq!(
            content_path(&torrent, &settings),
            format!("{root}Film{sep}")
        );
    }
    #[test]
    fn torrent_filters_distinguish_completed_paused_and_stalled_states() {
        let finished =
            json!({"state":"pausedUP","progress":1.0,"completion_on":100,"dlspeed":0,"upspeed":0});
        let pending = json!({"state":"downloading","progress":0.4,"completion_on":null,"dlspeed":0,"upspeed":0});
        let stalled = json!({"state":"stalledDL","progress":0.0,"completion_on":null,"dlspeed":0,"upspeed":0});
        assert!(matches_filter(&finished, Some("completed")));
        assert!(!matches_filter(&pending, Some("completed")));
        assert!(matches_filter(&finished, Some("paused")));
        assert!(matches_filter(&pending, Some("inactive")));
        assert!(matches_filter(&stalled, Some("stalled_downloading")));
        assert!(!matches_filter(&finished, Some("resumed")));
    }
}
